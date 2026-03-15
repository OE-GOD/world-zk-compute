// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title MockExecutionEngine
/// @notice Mock implementation of the ExecutionEngine interface for SDK unit tests.
/// @dev Allows configuring predetermined responses, tracks method call counts and
///      last call data, and emits the same events as the real ExecutionEngine.
contract MockExecutionEngine {
    // ========================================================================
    // TYPES (mirrored from ExecutionEngine)
    // ========================================================================

    enum RequestStatus {
        Pending,
        Claimed,
        Completed,
        Expired,
        Cancelled
    }

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
    // EVENTS (mirrored from ExecutionEngine)
    // ========================================================================

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

    event ExecutionClaimed(uint256 indexed requestId, address indexed prover, uint256 claimDeadline);
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, bytes32 journalDigest, uint256 payout);
    event ExecutionExpired(uint256 indexed requestId);
    event ExecutionCancelled(uint256 indexed requestId);
    event ClaimExpired(uint256 indexed requestId, address indexed prover);
    event CallbackFailed(uint256 indexed requestId, address indexed callbackContract);
    event ReputationContractSet(address indexed reputation);
    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);

    // ========================================================================
    // MOCK STATE
    // ========================================================================

    /// @notice Pre-configured responses: requestId -> result bytes
    mapping(bytes32 => bytes) private _responses;

    /// @notice Call counts per method name
    mapping(string => uint256) private _callCounts;

    /// @notice Last call data per method name
    mapping(string => bytes) private _lastCallData;

    /// @notice Stored requests (for getRequest)
    mapping(uint256 => ExecutionRequest) public requests;

    /// @notice Next request ID (starts at 1, matching real contract)
    uint256 public nextRequestId = 1;

    /// @notice Protocol fee basis points
    uint256 public protocolFeeBps = 250;

    /// @notice Fee recipient
    address public feeRecipient;

    /// @notice Prover stats
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    /// @notice Whether the mock should revert on submitProof
    bool public shouldRevertOnSubmitProof;
    string public submitProofRevertReason;

    // ========================================================================
    // MOCK CONFIGURATION
    // ========================================================================

    /// @notice Configure a predetermined response for a given request ID
    /// @param requestId The request ID to configure a response for
    /// @param result The result bytes to return/store
    function setResponse(bytes32 requestId, bytes calldata result) external {
        _responses[requestId] = result;
    }

    /// @notice Get the configured response for a request ID
    /// @param requestId The request ID to query
    /// @return The configured response bytes
    function getResponse(bytes32 requestId) external view returns (bytes memory) {
        return _responses[requestId];
    }

    /// @notice Get how many times a method was called
    /// @param methodName The method name to query (e.g., "requestExecution", "claimExecution")
    /// @return The number of times the method was called
    function getCallCount(string calldata methodName) external view returns (uint256) {
        return _callCounts[methodName];
    }

    /// @notice Get the last call's ABI-encoded arguments for a method
    /// @param methodName The method name to query
    /// @return The ABI-encoded arguments from the last call
    function getLastCallData(string calldata methodName) external view returns (bytes memory) {
        return _lastCallData[methodName];
    }

    /// @notice Configure the mock to revert on submitProof calls
    /// @param shouldRevert Whether submitProof should revert
    /// @param reason The revert reason string
    function setSubmitProofRevert(bool shouldRevert, string calldata reason) external {
        shouldRevertOnSubmitProof = shouldRevert;
        submitProofRevertReason = reason;
    }

    /// @notice Directly set a request in storage (for test setup)
    /// @param requestId The request ID
    /// @param req The request struct to store
    function setRequest(uint256 requestId, ExecutionRequest calldata req) external {
        requests[requestId] = req;
        if (requestId >= nextRequestId) {
            nextRequestId = requestId + 1;
        }
    }

    // ========================================================================
    // EXECUTION ENGINE INTERFACE (with call tracking + event emission)
    // ========================================================================

    /// @notice Mock requestExecution (6-param version with inputType)
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds,
        uint8 inputType
    ) external payable returns (uint256 requestId) {
        _callCounts["requestExecution"]++;
        _lastCallData["requestExecution"] =
            abi.encode(imageId, inputDigest, inputUrl, callbackContract, expirationSeconds, inputType);

        requestId = nextRequestId++;

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : 1 hours;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: RequestStatus.Pending,
            claimedBy: address(0),
            claimedAt: 0,
            claimDeadline: 0,
            tip: msg.value,
            maxTip: msg.value
        });

        emit ExecutionRequested(
            requestId, msg.sender, imageId, inputDigest, inputUrl, inputType, msg.value, block.timestamp + expiration
        );
    }

    /// @notice Mock requestExecution (5-param version, backward compatible)
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable returns (uint256 requestId) {
        _callCounts["requestExecution"]++;
        _lastCallData["requestExecution"] =
            abi.encode(imageId, inputDigest, inputUrl, callbackContract, expirationSeconds, uint8(0));

        requestId = nextRequestId++;

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : 1 hours;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: RequestStatus.Pending,
            claimedBy: address(0),
            claimedAt: 0,
            claimDeadline: 0,
            tip: msg.value,
            maxTip: msg.value
        });

        emit ExecutionRequested(
            requestId, msg.sender, imageId, inputDigest, inputUrl, 0, msg.value, block.timestamp + expiration
        );
    }

    /// @notice Mock cancelExecution
    function cancelExecution(uint256 requestId) external {
        _callCounts["cancelExecution"]++;
        _lastCallData["cancelExecution"] = abi.encode(requestId);

        ExecutionRequest storage req = requests[requestId];
        req.status = RequestStatus.Cancelled;

        // Refund tip
        if (req.tip > 0) {
            (bool success,) = payable(msg.sender).call{value: req.tip}("");
            require(success, "Transfer failed");
        }

        emit ExecutionCancelled(requestId);
    }

    /// @notice Mock claimExecution
    function claimExecution(uint256 requestId) external {
        _callCounts["claimExecution"]++;
        _lastCallData["claimExecution"] = abi.encode(requestId);

        ExecutionRequest storage req = requests[requestId];
        req.status = RequestStatus.Claimed;
        req.claimedBy = msg.sender;
        req.claimedAt = uint48(block.timestamp);
        req.claimDeadline = uint48(block.timestamp + 10 minutes);

        emit ExecutionClaimed(requestId, msg.sender, req.claimDeadline);
    }

    /// @notice Mock submitProof
    function submitProof(uint256 requestId, bytes calldata seal, bytes calldata journal) external {
        if (shouldRevertOnSubmitProof) {
            revert(submitProofRevertReason);
        }

        _callCounts["submitProof"]++;
        _lastCallData["submitProof"] = abi.encode(requestId, seal, journal);

        ExecutionRequest storage req = requests[requestId];
        req.status = RequestStatus.Completed;

        // Check for pre-configured response
        bytes memory response = _responses[bytes32(requestId)];
        if (response.length > 0) {
            // Store response as a completion marker (available via getResponse)
        }

        uint256 payout = req.tip;
        proverCompletedCount[msg.sender]++;
        proverEarnings[msg.sender] += payout;

        emit ExecutionCompleted(requestId, msg.sender, sha256(journal), payout);
    }

    // ========================================================================
    // VIEW FUNCTIONS (mirrored from ExecutionEngine)
    // ========================================================================

    /// @notice Get current tip for a request
    function getCurrentTip(uint256 requestId) public view returns (uint256) {
        ExecutionRequest storage req = requests[requestId];
        if (req.id == 0) return 0;
        if (req.status != RequestStatus.Pending && req.status != RequestStatus.Claimed) {
            return 0;
        }
        return req.tip;
    }

    /// @notice Get pending requests
    function getPendingRequests(uint256 offset, uint256 limit) external view returns (uint256[] memory) {
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

        uint256[] memory result = new uint256[](count);
        for (uint256 i = 0; i < count; i++) {
            result[i] = pending[i];
        }
        return result;
    }

    /// @notice Get request details
    function getRequest(uint256 requestId) external view returns (ExecutionRequest memory) {
        return requests[requestId];
    }

    /// @notice Get prover statistics
    function getProverStats(address prover) external view returns (uint256 completed, uint256 earnings) {
        return (proverCompletedCount[prover], proverEarnings[prover]);
    }

    // ========================================================================
    // RECEIVE (to accept ETH for tips)
    // ========================================================================

    receive() external payable {}
}
