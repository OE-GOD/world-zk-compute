// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "./ProgramRegistry.sol";

/// @title IRiscZeroVerifier
/// @notice Interface for RISC Zero's on-chain verifier
interface IRiscZeroVerifier {
    function verify(
        bytes calldata seal,
        bytes32 imageId,
        bytes32 journalDigest
    ) external view;
}

/// @title IExecutionCallback
/// @notice Interface for contracts that receive verified computation results
interface IExecutionCallback {
    function onExecutionComplete(
        uint256 requestId,
        bytes32 imageId,
        bytes calldata journal
    ) external;
}

/// @title ExecutionEngine
/// @notice Core engine for verifiable computation on World Chain
/// @dev Handles the full lifecycle: request → claim → prove → callback
contract ExecutionEngine {

    // ========================================================================
    // TYPES
    // ========================================================================

    enum RequestStatus {
        Pending,      // Waiting for prover to claim
        Claimed,      // Prover claimed, proof expected
        Completed,    // Proof verified, callback executed
        Expired,      // Claim expired, can be reclaimed
        Cancelled     // Requester cancelled
    }

    struct ExecutionRequest {
        uint256 id;
        address requester;
        bytes32 imageId;           // Program to execute
        bytes32 inputDigest;       // Hash of inputs (actual inputs stored off-chain/IPFS)
        string inputUrl;           // URL to fetch inputs
        address callbackContract;  // Where to send results
        uint256 tip;               // Payment for prover
        uint256 maxTip;            // Starting tip (decreases over time)
        uint256 createdAt;
        uint256 expiresAt;         // Request expiration
        RequestStatus status;
        address claimedBy;         // Prover who claimed
        uint256 claimedAt;
        uint256 claimDeadline;     // When claim expires if no proof
    }

    // ========================================================================
    // CONSTANTS
    // ========================================================================

    uint256 public constant MIN_TIP = 0.0001 ether;
    uint256 public constant DEFAULT_EXPIRATION = 1 hours;
    uint256 public constant CLAIM_WINDOW = 10 minutes;
    uint256 public constant TIP_DECAY_PERIOD = 30 minutes;

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice The program registry
    ProgramRegistry public immutable registry;

    /// @notice The RISC Zero verifier contract
    IRiscZeroVerifier public immutable verifier;

    /// @notice All execution requests
    mapping(uint256 => ExecutionRequest) public requests;

    /// @notice Next request ID
    uint256 public nextRequestId = 1;

    /// @notice Protocol fee (basis points)
    uint256 public protocolFeeBps = 250; // 2.5%

    /// @notice Fee recipient
    address public feeRecipient;

    /// @notice Prover stats
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        bytes32 inputDigest,
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

    // ========================================================================
    // ERRORS
    // ========================================================================

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
    error CallbackFailed();

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    constructor(
        address _registry,
        address _verifier,
        address _feeRecipient
    ) {
        registry = ProgramRegistry(_registry);
        verifier = IRiscZeroVerifier(_verifier);
        feeRecipient = _feeRecipient;
    }

    // ========================================================================
    // REQUEST EXECUTION
    // ========================================================================

    /// @notice Request execution of a zkVM program
    /// @param imageId The program to execute
    /// @param inputDigest Hash of the inputs
    /// @param inputUrl URL where prover can fetch inputs
    /// @param callbackContract Contract to receive results (0x0 for no callback)
    /// @param expirationSeconds How long before request expires
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable returns (uint256 requestId) {
        if (msg.value < MIN_TIP) revert InsufficientTip();
        if (!registry.isProgramActive(imageId)) revert ProgramNotActive();

        uint256 expiration = expirationSeconds > 0
            ? expirationSeconds
            : DEFAULT_EXPIRATION;

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            requester: msg.sender,
            imageId: imageId,
            inputDigest: inputDigest,
            inputUrl: inputUrl,
            callbackContract: callbackContract,
            tip: msg.value,
            maxTip: msg.value,
            createdAt: block.timestamp,
            expiresAt: block.timestamp + expiration,
            status: RequestStatus.Pending,
            claimedBy: address(0),
            claimedAt: 0,
            claimDeadline: 0
        });

        emit ExecutionRequested(
            requestId,
            msg.sender,
            imageId,
            inputDigest,
            msg.value,
            block.timestamp + expiration
        );
    }

    /// @notice Cancel a pending execution request
    function cancelExecution(uint256 requestId) external {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (req.requester != msg.sender) revert NotRequester();
        if (req.status != RequestStatus.Pending) revert RequestNotPending();

        req.status = RequestStatus.Cancelled;

        // Refund tip
        payable(msg.sender).transfer(req.tip);

        emit ExecutionCancelled(requestId);
    }

    // ========================================================================
    // PROVER OPERATIONS
    // ========================================================================

    /// @notice Claim an execution request (prover)
    /// @param requestId The request to claim
    function claimExecution(uint256 requestId) external {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (block.timestamp > req.expiresAt) revert RequestExpired();

        // If previously claimed but deadline passed, allow reclaim
        if (req.status == RequestStatus.Claimed) {
            if (block.timestamp <= req.claimDeadline) revert ClaimNotExpired();
            emit ClaimExpired(requestId, req.claimedBy);
        } else if (req.status != RequestStatus.Pending) {
            revert RequestNotPending();
        }

        req.status = RequestStatus.Claimed;
        req.claimedBy = msg.sender;
        req.claimedAt = block.timestamp;
        req.claimDeadline = block.timestamp + CLAIM_WINDOW;

        emit ExecutionClaimed(requestId, msg.sender, req.claimDeadline);
    }

    /// @notice Submit proof for a claimed execution
    /// @param requestId The request ID
    /// @param seal The RISC Zero proof seal
    /// @param journal The public outputs (journal)
    function submitProof(
        uint256 requestId,
        bytes calldata seal,
        bytes calldata journal
    ) external {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) revert RequestNotFound();
        if (req.status != RequestStatus.Claimed) revert RequestNotClaimed();
        if (req.claimedBy != msg.sender) revert NotClaimant();
        if (block.timestamp > req.claimDeadline) revert ClaimDeadlinePassed();

        // Verify the proof
        bytes32 journalDigest = sha256(journal);

        // This will revert if proof is invalid
        verifier.verify(seal, req.imageId, journalDigest);

        // Mark as completed
        req.status = RequestStatus.Completed;

        // Calculate payout with tip decay
        uint256 payout = calculatePayout(req);
        uint256 fee = (payout * protocolFeeBps) / 10000;
        uint256 proverPayout = payout - fee;

        // Update prover stats
        proverCompletedCount[msg.sender]++;
        proverEarnings[msg.sender] += proverPayout;

        // Pay prover
        payable(msg.sender).transfer(proverPayout);

        // Pay protocol fee
        if (fee > 0) {
            payable(feeRecipient).transfer(fee);
        }

        emit ExecutionCompleted(requestId, msg.sender, journalDigest, proverPayout);

        // Execute callback if specified
        if (req.callbackContract != address(0)) {
            try IExecutionCallback(req.callbackContract).onExecutionComplete(
                requestId,
                req.imageId,
                journal
            ) {} catch {
                // Callback failed but proof is still valid
                // Could emit an event here
            }
        }
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get current tip for a request (decreases over time)
    function getCurrentTip(uint256 requestId) public view returns (uint256) {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) return 0;
        if (req.status != RequestStatus.Pending && req.status != RequestStatus.Claimed) {
            return 0;
        }

        return calculatePayout(req);
    }

    /// @notice Calculate payout with tip decay
    function calculatePayout(ExecutionRequest storage req) internal view returns (uint256) {
        uint256 elapsed = block.timestamp - req.createdAt;

        if (elapsed >= TIP_DECAY_PERIOD) {
            // After decay period, tip is at minimum (50% of max)
            return req.maxTip / 2;
        }

        // Linear decay from maxTip to maxTip/2
        uint256 decay = (req.maxTip * elapsed) / (TIP_DECAY_PERIOD * 2);
        return req.maxTip - decay;
    }

    /// @notice Get pending requests (for prover to find work)
    function getPendingRequests(uint256 offset, uint256 limit)
        external
        view
        returns (uint256[] memory)
    {
        uint256[] memory pending = new uint256[](limit);
        uint256 count = 0;
        uint256 checked = 0;

        for (uint256 i = 1; i < nextRequestId && count < limit; i++) {
            ExecutionRequest storage req = requests[i];

            if (req.status == RequestStatus.Pending && block.timestamp <= req.expiresAt) {
                if (checked >= offset) {
                    pending[count++] = i;
                }
                checked++;
            }
        }

        // Resize array
        uint256[] memory result = new uint256[](count);
        for (uint256 i = 0; i < count; i++) {
            result[i] = pending[i];
        }

        return result;
    }

    /// @notice Get request details
    function getRequest(uint256 requestId) external view returns (ExecutionRequest memory) {
        if (requests[requestId].id == 0) revert RequestNotFound();
        return requests[requestId];
    }

    /// @notice Get prover statistics
    function getProverStats(address prover)
        external
        view
        returns (uint256 completed, uint256 earnings)
    {
        return (proverCompletedCount[prover], proverEarnings[prover]);
    }

    // ========================================================================
    // ADMIN
    // ========================================================================

    function setProtocolFee(uint256 _feeBps) external {
        require(msg.sender == feeRecipient, "Not authorized");
        require(_feeBps <= 1000, "Fee too high"); // Max 10%
        protocolFeeBps = _feeBps;
    }

    function setFeeRecipient(address _recipient) external {
        require(msg.sender == feeRecipient, "Not authorized");
        feeRecipient = _recipient;
    }
}
