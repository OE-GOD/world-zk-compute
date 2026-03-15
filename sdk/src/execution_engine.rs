//! ExecutionEngine SDK bindings.
//!
//! Provides a Rust client for the `ExecutionEngine` contract, which manages
//! the full lifecycle of verifiable computation requests: submit, claim, prove,
//! and query.
//!
//! # Example
//!
//! ```rust,no_run
//! use world_zk_sdk::{Client, ExecutionEngineClient};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let client = Client::new(
//!         "http://localhost:8545",
//!         "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
//!         "0x5FbDB2315678afecb367f032d93F642f64180aa3",
//!     )?;
//!
//!     let engine = ExecutionEngineClient::new(client);
//!
//!     // Query pending requests
//!     let pending = engine.get_pending_requests(0, 10).await?;
//!     println!("Pending requests: {:?}", pending);
//!
//!     Ok(())
//! }
//! ```

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;

use crate::client::Client;

// ---------------------------------------------------------------------------
// ABI bindings generated via alloy sol! macro
// ---------------------------------------------------------------------------

sol! {
    #[sol(rpc)]
    contract ExecutionEngine {
        // --- Enums ---
        // Note: Solidity enums are represented as uint8 on the ABI level.

        // --- Structs ---

        struct ExecutionRequest {
            uint256 id;
            bytes32 imageId;
            bytes32 inputDigest;
            address requester;
            uint48 createdAt;
            uint48 expiresAt;
            address callbackContract;
            uint8 status;   // RequestStatus enum: 0=Pending, 1=Claimed, 2=Completed, 3=Expired, 4=Cancelled
            address claimedBy;
            uint48 claimedAt;
            uint48 claimDeadline;
            uint256 tip;
            uint256 maxTip;
        }

        // --- Events ---

        event ExecutionRequested(
            uint256 indexed requestId,
            address indexed requester,
            bytes32 indexed imageId,
            bytes32 inputDigest,
            string inputUrl,
            uint8 inputType,
            uint256 tip,
            uint256 expiresAt
        );

        event ExecutionClaimed(
            uint256 indexed requestId,
            address indexed prover,
            uint256 claimDeadline
        );

        event ExecutionCompleted(
            uint256 indexed requestId,
            address indexed prover,
            bytes32 journalDigest,
            uint256 payout
        );

        event ExecutionExpired(uint256 indexed requestId);
        event ExecutionCancelled(uint256 indexed requestId);
        event ClaimExpired(uint256 indexed requestId, address indexed prover);
        event CallbackFailed(uint256 indexed requestId, address indexed callbackContract);

        // --- Errors ---

        error InsufficientTip();
        error ProgramNotActive();
        error RequestNotFound();
        error RequestNotPending();
        error RequestNotClaimed();
        error NotRequester();
        error NotClaimant();
        error ClaimNotExpired();
        error RequestExpired();
        error ClaimDeadlinePassed();
        error InvalidProof();
        error EmptySeal();
        error EmptyJournal();
        error TransferFailed();
        error ZeroImageId();
        error ExpirationInPast();
        error ZeroAddress();

        // --- Mutating Functions ---

        function requestExecution(
            bytes32 imageId,
            bytes32 inputDigest,
            string calldata inputUrl,
            address callbackContract,
            uint256 expirationSeconds,
            uint8 inputType
        ) external payable returns (uint256 requestId);

        function cancelExecution(uint256 requestId) external;

        function claimExecution(uint256 requestId) external;

        function submitProof(
            uint256 requestId,
            bytes calldata seal,
            bytes calldata journal
        ) external;

        // --- Admin Functions ---

        function pause() external;
        function unpause() external;
        function setProtocolFee(uint256 feeBps) external;
        function setFeeRecipient(address recipient) external;
        function setReputation(address reputation) external;

        // --- View Functions ---

        function getRequest(uint256 requestId) external view returns (ExecutionRequest memory);
        function getCurrentTip(uint256 requestId) external view returns (uint256);
        function getPendingRequests(uint256 offset, uint256 limit) external view returns (uint256[] memory);
        function getProverStats(address prover) external view returns (uint256 completed, uint256 earnings);

        // --- Constants ---
        function MIN_TIP() external view returns (uint256);
        function DEFAULT_EXPIRATION() external view returns (uint256);
        function CLAIM_WINDOW() external view returns (uint256);
        function TIP_DECAY_PERIOD() external view returns (uint256);

        // --- State ---
        function nextRequestId() external view returns (uint256);
        function protocolFeeBps() external view returns (uint256);
        function feeRecipient() external view returns (address);
    }
}

// ---------------------------------------------------------------------------
// Request status enum (mirrors Solidity's RequestStatus)
// ---------------------------------------------------------------------------

/// Lifecycle status of an execution request.
///
/// Mirrors the Solidity `RequestStatus` enum in `ExecutionEngine.sol`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RequestStatus {
    /// Waiting for a prover to claim.
    Pending = 0,
    /// Prover claimed, proof expected within the claim window.
    Claimed = 1,
    /// Proof verified, callback executed, prover paid.
    Completed = 2,
    /// Claim expired, can be reclaimed by another prover.
    Expired = 3,
    /// Requester cancelled the request and was refunded.
    Cancelled = 4,
}

impl RequestStatus {
    /// Convert from the on-chain `uint8` representation.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Pending),
            1 => Some(Self::Claimed),
            2 => Some(Self::Completed),
            3 => Some(Self::Expired),
            4 => Some(Self::Cancelled),
            _ => None,
        }
    }
}

impl std::fmt::Display for RequestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "Pending"),
            Self::Claimed => write!(f, "Claimed"),
            Self::Completed => write!(f, "Completed"),
            Self::Expired => write!(f, "Expired"),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

// ---------------------------------------------------------------------------
// ExecutionEngineClient
// ---------------------------------------------------------------------------

/// Client for the `ExecutionEngine` contract.
///
/// Wraps the alloy-generated bindings and provides ergonomic methods for
/// the full request lifecycle: submit, claim, prove, query, and admin.
pub struct ExecutionEngineClient {
    client: Client,
}

impl ExecutionEngineClient {
    /// Create a new `ExecutionEngineClient` from an SDK `Client`.
    ///
    /// The client's contract address should point to a deployed `ExecutionEngine`.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Returns a reference to the underlying SDK client.
    pub fn client(&self) -> &Client {
        &self.client
    }

    // =====================================================================
    // Request Submission
    // =====================================================================

    /// Submit a new execution request.
    ///
    /// Calls `requestExecution` on-chain with the given parameters and attached
    /// tip (sent as `msg.value`).
    ///
    /// # Arguments
    ///
    /// * `image_id` - The zkVM program image ID to execute.
    /// * `input_digest` - Hash of the inputs (actual inputs stored off-chain).
    /// * `input_url` - URL where the prover can fetch inputs.
    /// * `callback_contract` - Contract to receive results via `onExecutionComplete`
    ///   (use `Address::ZERO` for no callback).
    /// * `expiration_seconds` - How long before request expires (0 uses contract default).
    /// * `input_type` - 0 = Public, 1 = Private (event-only, not stored).
    /// * `tip` - ETH tip to attach (must be >= `MIN_TIP`).
    ///
    /// # Returns
    ///
    /// The transaction hash of the submitted request.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(image_id = %image_id, input_digest = %input_digest)))]
    pub async fn submit_request(
        &self,
        image_id: B256,
        input_digest: B256,
        input_url: &str,
        callback_contract: Address,
        expiration_seconds: u64,
        input_type: u8,
        tip: U256,
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .requestExecution(
                image_id,
                input_digest,
                input_url.to_string(),
                callback_contract,
                U256::from(expiration_seconds),
                input_type,
            )
            .value(tip)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Cancel a pending execution request and receive a tip refund.
    ///
    /// Only the original requester can cancel. The request must still be in
    /// `Pending` status.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn cancel_request(&self, request_id: u64) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .cancelExecution(U256::from(request_id))
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Prover Operations
    // =====================================================================

    /// Claim a pending execution request (prover operation).
    ///
    /// After claiming, the prover has `CLAIM_WINDOW` (10 minutes) to submit
    /// a valid proof via `submit_result`.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn claim_job(&self, request_id: u64) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .claimExecution(U256::from(request_id))
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Submit a proof for a claimed execution request.
    ///
    /// The proof is routed to the appropriate verifier (RISC Zero or custom)
    /// based on the program's registry entry. On success, the prover is paid
    /// the current tip (with decay) minus the protocol fee.
    ///
    /// # Arguments
    ///
    /// * `request_id` - The request ID that was previously claimed.
    /// * `seal` - The proof seal (RISC Zero seal or custom verifier proof).
    /// * `journal` - The public outputs (journal / public inputs).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn submit_result(
        &self,
        request_id: u64,
        seal: &[u8],
        journal: &[u8],
    ) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .submitProof(
                U256::from(request_id),
                Bytes::copy_from_slice(seal),
                Bytes::copy_from_slice(journal),
            )
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Query Functions
    // =====================================================================

    /// Get the full execution request details.
    ///
    /// Returns the on-chain `ExecutionRequest` struct for the given request ID.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn get_request(
        &self,
        request_id: u64,
    ) -> anyhow::Result<ExecutionEngine::ExecutionRequest> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let req = contract
            .getRequest(U256::from(request_id))
            .call()
            .await?;

        Ok(req)
    }

    /// Get the status of an execution request.
    ///
    /// Returns a parsed `RequestStatus` enum value.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn get_request_status(&self, request_id: u64) -> anyhow::Result<RequestStatus> {
        let req = self.get_request(request_id).await?;
        RequestStatus::from_u8(req.status).ok_or_else(|| {
            anyhow::anyhow!("unknown request status: {}", req.status)
        })
    }

    /// Get the current tip for a request (decreases over time via linear decay).
    ///
    /// Returns 0 if the request is completed or cancelled.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn get_current_tip(&self, request_id: u64) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let tip = contract
            .getCurrentTip(U256::from(request_id))
            .call()
            .await?;

        Ok(tip)
    }

    /// Get a list of pending (unclaimed, unexpired) request IDs.
    ///
    /// # Arguments
    ///
    /// * `offset` - Number of matching requests to skip.
    /// * `limit` - Maximum number of request IDs to return.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(offset, limit)))]
    pub async fn get_pending_requests(
        &self,
        offset: u64,
        limit: u64,
    ) -> anyhow::Result<Vec<U256>> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let ids = contract
            .getPendingRequests(U256::from(offset), U256::from(limit))
            .call()
            .await?;

        Ok(ids)
    }

    /// Get prover statistics (completed count and total earnings).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(prover = %prover)))]
    pub async fn get_prover_stats(
        &self,
        prover: Address,
    ) -> anyhow::Result<(U256, U256)> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let result = contract.getProverStats(prover).call().await?;
        Ok((result.completed, result.earnings))
    }

    /// Get the next request ID that will be assigned.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn next_request_id(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let id = contract.nextRequestId().call().await?;
        Ok(id)
    }

    /// Get the current protocol fee in basis points (100 = 1%).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn protocol_fee_bps(&self) -> anyhow::Result<U256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let fee = contract.protocolFeeBps().call().await?;
        Ok(fee)
    }

    /// Get the fee recipient address.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn fee_recipient(&self) -> anyhow::Result<Address> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let addr = contract.feeRecipient().call().await?;
        Ok(addr)
    }

    // =====================================================================
    // Admin Functions
    // =====================================================================

    /// Pause the contract (owner only).
    ///
    /// Blocks `requestExecution`, `claimExecution`, and `submitProof`.
    /// `cancelExecution` remains available so users can recover funds.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn pause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract.pause().send().await?.get_receipt().await?;
        Ok(receipt.transaction_hash)
    }

    /// Unpause the contract (owner only).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
    pub async fn unpause(&self) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract.unpause().send().await?.get_receipt().await?;
        Ok(receipt.transaction_hash)
    }

    /// Set the protocol fee in basis points (owner only, max 10% = 1000 bps).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(fee_bps)))]
    pub async fn set_protocol_fee(&self, fee_bps: u64) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .setProtocolFee(U256::from(fee_bps))
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Set the fee recipient address (owner only).
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(recipient = %recipient)))]
    pub async fn set_fee_recipient(&self, recipient: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .setFeeRecipient(recipient)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    /// Set or update the reputation contract (owner only).
    ///
    /// Pass `Address::ZERO` to disable reputation tracking.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(reputation = %reputation)))]
    pub async fn set_reputation(&self, reputation: Address) -> anyhow::Result<B256> {
        let provider = self.build_provider();
        let contract = ExecutionEngine::new(self.client.contract_address(), provider);

        let receipt = contract
            .setReputation(reputation)
            .send()
            .await?
            .get_receipt()
            .await?;

        Ok(receipt.transaction_hash)
    }

    // =====================================================================
    // Dispute (convenience wrapper)
    // =====================================================================

    /// Initiate a dispute by reclaiming an expired job.
    ///
    /// If a prover's claim has expired without proof submission, any address
    /// can call `claimExecution` to reclaim the job. This method is a semantic
    /// alias for `claim_job` that makes the dispute-reclaim intent explicit.
    ///
    /// For TEE-style disputes (challenge/resolve), use the `TEEVerifier` client.
    #[cfg_attr(feature = "tracing", tracing::instrument(skip_all, fields(request_id)))]
    pub async fn dispute_result(&self, request_id: u64) -> anyhow::Result<B256> {
        // In the ExecutionEngine contract, disputing an expired claim is done
        // by calling claimExecution again after the claim deadline has passed.
        // The contract checks if the claim has expired and allows reclaim.
        self.claim_job(request_id).await
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
    use alloy::primitives::{Address, B256, U256};

    fn test_client() -> Client {
        Client::new(
            "http://localhost:8545",
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
            "0x5FbDB2315678afecb367f032d93F642f64180aa3",
        )
        .expect("should create test client")
    }

    #[test]
    fn test_execution_engine_client_construction() {
        let client = test_client();
        let engine = ExecutionEngineClient::new(client);
        assert_eq!(
            engine.client().contract_address(),
            "0x5FbDB2315678afecb367f032d93F642f64180aa3"
                .parse::<Address>()
                .unwrap()
        );
    }

    #[test]
    fn test_execution_engine_client_signer_address() {
        let client = test_client();
        let signer = client.signer_address();
        let engine = ExecutionEngineClient::new(client);
        // The Anvil default account #0
        assert_eq!(engine.client().signer_address(), signer);
        assert_ne!(engine.client().signer_address(), Address::ZERO);
    }

    #[test]
    fn test_request_status_from_u8() {
        assert_eq!(RequestStatus::from_u8(0), Some(RequestStatus::Pending));
        assert_eq!(RequestStatus::from_u8(1), Some(RequestStatus::Claimed));
        assert_eq!(RequestStatus::from_u8(2), Some(RequestStatus::Completed));
        assert_eq!(RequestStatus::from_u8(3), Some(RequestStatus::Expired));
        assert_eq!(RequestStatus::from_u8(4), Some(RequestStatus::Cancelled));
        assert_eq!(RequestStatus::from_u8(5), None);
        assert_eq!(RequestStatus::from_u8(255), None);
    }

    #[test]
    fn test_request_status_display() {
        assert_eq!(format!("{}", RequestStatus::Pending), "Pending");
        assert_eq!(format!("{}", RequestStatus::Claimed), "Claimed");
        assert_eq!(format!("{}", RequestStatus::Completed), "Completed");
        assert_eq!(format!("{}", RequestStatus::Expired), "Expired");
        assert_eq!(format!("{}", RequestStatus::Cancelled), "Cancelled");
    }

    #[test]
    fn test_request_status_equality() {
        assert_eq!(RequestStatus::Pending, RequestStatus::Pending);
        assert_ne!(RequestStatus::Pending, RequestStatus::Claimed);
    }

    #[test]
    fn test_request_status_clone_copy() {
        let status = RequestStatus::Claimed;
        let cloned = status;
        assert_eq!(status, cloned);
    }

    #[test]
    fn test_request_status_debug() {
        let debug = format!("{:?}", RequestStatus::Completed);
        assert!(debug.contains("Completed"));
    }

    #[test]
    fn test_request_status_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(RequestStatus::Pending);
        set.insert(RequestStatus::Claimed);
        set.insert(RequestStatus::Pending); // duplicate
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_execution_request_struct_fields() {
        // Verify the generated struct has expected fields by constructing one.
        let req = ExecutionEngine::ExecutionRequest {
            id: U256::from(1),
            imageId: B256::from([0xaa; 32]),
            inputDigest: B256::from([0xbb; 32]),
            requester: Address::ZERO,
            createdAt: alloy::primitives::Uint::from(1000u64),
            expiresAt: alloy::primitives::Uint::from(2000u64),
            callbackContract: Address::ZERO,
            status: 0, // Pending
            claimedBy: Address::ZERO,
            claimedAt: alloy::primitives::Uint::from(0u64),
            claimDeadline: alloy::primitives::Uint::from(0u64),
            tip: U256::from(100_000_000_000_000u64), // 0.0001 ETH
            maxTip: U256::from(100_000_000_000_000u64),
        };

        assert_eq!(req.id, U256::from(1));
        assert_eq!(req.imageId, B256::from([0xaa; 32]));
        assert_eq!(req.status, 0);
        assert_eq!(req.createdAt, alloy::primitives::Uint::from(1000u64));
        assert_eq!(req.expiresAt, alloy::primitives::Uint::from(2000u64));
    }

    #[test]
    fn test_multiple_clients_from_different_keys() {
        // Verify that different private keys produce different signer addresses.
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

        let engine1 = ExecutionEngineClient::new(client1);
        let engine2 = ExecutionEngineClient::new(client2);

        assert_ne!(
            engine1.client().signer_address(),
            engine2.client().signer_address()
        );
    }

    #[test]
    fn test_request_status_repr() {
        // Verify repr(u8) gives the expected numeric values.
        assert_eq!(RequestStatus::Pending as u8, 0);
        assert_eq!(RequestStatus::Claimed as u8, 1);
        assert_eq!(RequestStatus::Completed as u8, 2);
        assert_eq!(RequestStatus::Expired as u8, 3);
        assert_eq!(RequestStatus::Cancelled as u8, 4);
    }
}
