// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title IExecutionEngine -- Interface for the verifiable computation engine
/// @notice Defines the public API for requesting, claiming, proving, and managing
///         zkVM program executions on World Chain.
interface IExecutionEngine {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Lifecycle status of an execution request
    enum RequestStatus {
        Pending, // Waiting for prover to claim
        Claimed, // Prover claimed, proof expected
        Completed, // Proof verified, callback executed
        Expired, // Claim expired, can be reclaimed
        Cancelled // Requester cancelled
    }

    /// @notice Storage layout for a single execution request
    struct ExecutionRequest {
        uint256 id;
        bytes32 imageId;
        bytes32 inputDigest;
        address requester;
        uint48 createdAt;
        uint48 expiresAt;
        address callbackContract;
        RequestStatus status;
        address claimedBy;
        uint48 claimedAt;
        uint48 claimDeadline;
        uint256 tip;
        uint256 maxTip;
    }

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when a new execution request is created
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

    /// @notice Emitted when a prover claims an execution request
    event ExecutionClaimed(uint256 indexed requestId, address indexed prover, uint256 claimDeadline);

    /// @notice Emitted when a proof is verified and the prover is paid
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, bytes32 journalDigest, uint256 payout);

    /// @notice Emitted when a request passes its expiration time
    event ExecutionExpired(uint256 indexed requestId);

    /// @notice Emitted when a requester cancels their pending request
    event ExecutionCancelled(uint256 indexed requestId);

    /// @notice Emitted when a prover's claim deadline passes without proof submission
    event ClaimExpired(uint256 indexed requestId, address indexed prover);

    /// @notice Emitted when an onExecutionComplete callback reverts (proof still valid)
    event CallbackFailed(uint256 indexed requestId, address indexed callbackContract);

    /// @notice Emitted when the reputation contract address is updated
    event ReputationContractSet(address indexed reputation);

    /// @notice Emitted when the protocol fee basis points are changed
    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);

    /// @notice Emitted when the fee recipient address is changed
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);

    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Thrown when msg.value is below MIN_TIP
    error InsufficientTip();

    /// @notice Thrown when the requested program is not active in the registry
    error ProgramNotActive();

    /// @notice Thrown when the specified request ID does not exist
    error RequestNotFound();

    /// @notice Thrown when the request is not in Pending status
    error RequestNotPending();

    /// @notice Thrown when the request is not in Claimed status
    error RequestNotClaimed();

    /// @notice Thrown when msg.sender is not the original requester
    error NotRequester();

    /// @notice Thrown when msg.sender is not the prover who claimed the request
    error NotClaimant();

    /// @notice Thrown when trying to reclaim a request whose claim has not yet expired
    error ClaimNotExpired();

    /// @notice Thrown when trying to claim a request past its expiration
    error RequestExpired();

    /// @notice Thrown when proof is submitted after the claim deadline
    error ClaimDeadlinePassed();

    /// @notice Thrown when the proof verification fails
    error InvalidProof();

    /// @notice Thrown when an empty seal is provided to submitProof
    error EmptySeal();

    /// @notice Thrown when an empty journal is provided to submitProof
    error EmptyJournal();

    /// @notice Thrown when an ETH transfer (tip refund or payout) fails
    error TransferFailed();

    /// @notice Thrown when imageId is bytes32(0)
    error ZeroImageId();

    /// @notice Thrown when the computed expiration overflows
    error ExpirationInPast();

    /// @notice Thrown when a zero address is provided where a valid address is required
    error ZeroAddress();

    // ========================================================================
    // EXTERNAL FUNCTIONS
    // ========================================================================

    /// @notice Request execution of a zkVM program
    /// @param imageId The program to execute
    /// @param inputDigest Hash of the inputs
    /// @param inputUrl URL where prover can fetch inputs
    /// @param callbackContract Contract to receive results (0x0 for no callback)
    /// @param expirationSeconds How long before request expires (0 uses DEFAULT_EXPIRATION)
    /// @param inputType 0 = Public, 1 = Private (event-only, not stored)
    /// @return requestId The unique ID assigned to this request
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds,
        uint8 inputType
    ) external payable returns (uint256 requestId);

    /// @notice Request execution with public input (backward-compatible overload)
    /// @param imageId The program to execute
    /// @param inputDigest Hash of the inputs
    /// @param inputUrl URL where prover can fetch inputs
    /// @param callbackContract Contract to receive results (0x0 for no callback)
    /// @param expirationSeconds How long before request expires (0 uses DEFAULT_EXPIRATION)
    /// @return requestId The unique ID assigned to this request
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable returns (uint256 requestId);

    /// @notice Cancel a pending execution request and refund the tip
    /// @param requestId The request to cancel (must be Pending and owned by msg.sender)
    function cancelExecution(uint256 requestId) external;

    /// @notice Claim an execution request (prover)
    /// @param requestId The request to claim
    function claimExecution(uint256 requestId) external;

    /// @notice Submit proof for a claimed execution
    /// @param requestId The request ID
    /// @param seal The proof seal (risc0 seal or Remainder proof)
    /// @param journal The public outputs (journal / public inputs)
    function submitProof(uint256 requestId, bytes calldata seal, bytes calldata journal) external;

    /// @notice Get current tip for a request (decreases over time via linear decay)
    /// @param requestId The request to query
    /// @return The current tip amount in wei (0 if request is completed/cancelled)
    function getCurrentTip(uint256 requestId) external view returns (uint256);

    /// @notice Get pending requests (for prover to find work)
    /// @param offset Number of matching requests to skip before returning
    /// @param limit Maximum number of request IDs to return
    /// @return Array of request IDs that are currently Pending and not expired
    function getPendingRequests(uint256 offset, uint256 limit) external view returns (uint256[] memory);

    /// @notice Get request details
    /// @param requestId The request to query
    /// @return The full ExecutionRequest struct
    function getRequest(uint256 requestId) external view returns (ExecutionRequest memory);

    /// @notice Get prover statistics
    /// @param prover The prover address to query
    /// @return completed Number of proofs successfully submitted
    /// @return earnings Total ETH earned from payouts
    function getProverStats(address prover) external view returns (uint256 completed, uint256 earnings);

    /// @notice Pause the contract (blocks createRequest, claimRequest, submitProof)
    function pause() external;

    /// @notice Unpause the contract
    function unpause() external;

    /// @notice Update the protocol fee in basis points (max 10%)
    /// @param _feeBps New fee in basis points (100 = 1%, max 1000 = 10%)
    function setProtocolFee(uint256 _feeBps) external;

    /// @notice Update the fee recipient address
    /// @param _recipient New address to receive protocol fees (must be non-zero)
    function setFeeRecipient(address _recipient) external;

    /// @notice Set or update the reputation contract
    /// @param _reputation ProverReputation contract address (0x0 to disable)
    function setReputation(address _reputation) external;
}
