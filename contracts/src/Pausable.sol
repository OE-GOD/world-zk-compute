// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title Pausable
/// @notice Emergency pause functionality for critical contracts
/// @dev Inherit this contract and use `whenNotPaused` modifier on critical functions
abstract contract Pausable {
    // ========================================================================
    // EVENTS
    // ========================================================================

    event Paused(address indexed by, string reason);
    event Unpaused(address indexed by);
    event GuardianAdded(address indexed guardian);
    event GuardianRemoved(address indexed guardian);
    event PauseThresholdChanged(uint256 oldThreshold, uint256 newThreshold);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error ContractPaused();
    error ContractNotPaused();
    error NotGuardian();
    error NotOwner();
    error AlreadyGuardian();
    error NotAGuardian();
    error InvalidThreshold();
    error AlreadyVoted();
    error ThresholdNotMet();
    error ZeroAddress();

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice Whether the contract is paused
    bool public paused;

    /// @notice Reason for pause (for transparency)
    string public pauseReason;

    /// @notice When the contract was paused
    uint256 public pausedAt;

    /// @notice Contract owner (can add/remove guardians)
    address public owner;

    /// @notice Guardians who can pause (multi-sig style)
    mapping(address => bool) public isGuardian;
    address[] public guardians;

    /// @notice Number of guardians required to unpause
    uint256 public unpauseThreshold;

    /// @notice Votes to unpause
    mapping(address => bool) public unpauseVotes;
    uint256 public unpauseVoteCount;

    // ========================================================================
    // MODIFIERS
    // ========================================================================

    /// @notice Reverts if the contract is paused
    modifier whenNotPaused() {
        if (paused) revert ContractPaused();
        _;
    }

    /// @notice Reverts if the contract is not paused
    modifier whenPaused() {
        if (!paused) revert ContractNotPaused();
        _;
    }

    /// @notice Only guardians can call
    modifier onlyGuardian() {
        if (!isGuardian[msg.sender]) revert NotGuardian();
        _;
    }

    /// @notice Only owner can call
    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @notice Initialize pausable with owner and initial guardians
    /// @param _owner Contract owner
    /// @param _guardians Initial guardian addresses
    /// @param _unpauseThreshold Number of guardians required to unpause
    function __Pausable_init(
        address _owner,
        address[] memory _guardians,
        uint256 _unpauseThreshold
    ) internal {
        if (_owner == address(0)) revert ZeroAddress();
        if (_unpauseThreshold == 0 || _unpauseThreshold > _guardians.length) {
            revert InvalidThreshold();
        }

        owner = _owner;
        unpauseThreshold = _unpauseThreshold;

        for (uint256 i = 0; i < _guardians.length; i++) {
            address guardian = _guardians[i];
            if (guardian == address(0)) revert ZeroAddress();
            if (isGuardian[guardian]) revert AlreadyGuardian();

            isGuardian[guardian] = true;
            guardians.push(guardian);
            emit GuardianAdded(guardian);
        }
    }

    // ========================================================================
    // PAUSE FUNCTIONS
    // ========================================================================

    /// @notice Pause the contract (any guardian can pause immediately)
    /// @param reason Reason for pausing (for transparency/auditing)
    function pause(string calldata reason) external onlyGuardian whenNotPaused {
        paused = true;
        pauseReason = reason;
        pausedAt = block.timestamp;

        // Reset unpause votes
        _resetUnpauseVotes();

        emit Paused(msg.sender, reason);
    }

    /// @notice Vote to unpause the contract (requires threshold)
    function voteToUnpause() external onlyGuardian whenPaused {
        if (unpauseVotes[msg.sender]) revert AlreadyVoted();

        unpauseVotes[msg.sender] = true;
        unpauseVoteCount++;

        // Check if threshold met
        if (unpauseVoteCount >= unpauseThreshold) {
            _unpause();
        }
    }

    /// @notice Emergency unpause by owner (bypass multi-sig)
    /// @dev Use only in extreme circumstances
    function emergencyUnpause() external onlyOwner whenPaused {
        _unpause();
    }

    /// @notice Internal unpause logic
    function _unpause() internal {
        paused = false;
        pauseReason = "";
        pausedAt = 0;
        _resetUnpauseVotes();

        emit Unpaused(msg.sender);
    }

    /// @notice Reset all unpause votes
    function _resetUnpauseVotes() internal {
        for (uint256 i = 0; i < guardians.length; i++) {
            unpauseVotes[guardians[i]] = false;
        }
        unpauseVoteCount = 0;
    }

    // ========================================================================
    // GUARDIAN MANAGEMENT
    // ========================================================================

    /// @notice Add a new guardian
    function addGuardian(address guardian) external onlyOwner {
        if (guardian == address(0)) revert ZeroAddress();
        if (isGuardian[guardian]) revert AlreadyGuardian();

        isGuardian[guardian] = true;
        guardians.push(guardian);

        emit GuardianAdded(guardian);
    }

    /// @notice Remove a guardian
    function removeGuardian(address guardian) external onlyOwner {
        if (!isGuardian[guardian]) revert NotAGuardian();

        // Ensure we maintain minimum threshold
        if (guardians.length - 1 < unpauseThreshold) revert InvalidThreshold();

        isGuardian[guardian] = false;

        // Remove from array
        for (uint256 i = 0; i < guardians.length; i++) {
            if (guardians[i] == guardian) {
                guardians[i] = guardians[guardians.length - 1];
                guardians.pop();
                break;
            }
        }

        // Clear any existing vote
        if (unpauseVotes[guardian]) {
            unpauseVotes[guardian] = false;
            unpauseVoteCount--;
        }

        emit GuardianRemoved(guardian);
    }

    /// @notice Change the unpause threshold
    function setUnpauseThreshold(uint256 newThreshold) external onlyOwner {
        if (newThreshold == 0 || newThreshold > guardians.length) {
            revert InvalidThreshold();
        }

        emit PauseThresholdChanged(unpauseThreshold, newThreshold);
        unpauseThreshold = newThreshold;
    }

    /// @notice Transfer ownership
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get all guardians
    function getGuardians() external view returns (address[] memory) {
        return guardians;
    }

    /// @notice Get number of guardians
    function guardianCount() external view returns (uint256) {
        return guardians.length;
    }

    /// @notice Check if enough votes to unpause
    function canUnpause() external view returns (bool) {
        return unpauseVoteCount >= unpauseThreshold;
    }

    /// @notice Get pause status details
    function getPauseStatus()
        external
        view
        returns (
            bool isPaused,
            string memory reason,
            uint256 pauseTime,
            uint256 votesToUnpause,
            uint256 threshold
        )
    {
        return (paused, pauseReason, pausedAt, unpauseVoteCount, unpauseThreshold);
    }
}


/// @title PausableExecutionEngine
/// @notice ExecutionEngine with emergency pause capability
/// @dev Wrapper that adds pause functionality to critical operations
contract PausableExecutionEngine is Pausable {
    // Import the base ExecutionEngine interface
    // In production, you would inherit from ExecutionEngine directly

    // For this example, we demonstrate the pattern:

    /// @notice Request execution (pausable)
    /// @dev Add `whenNotPaused` to prevent new requests during emergency
    // function requestExecution(...) external payable whenNotPaused { ... }

    /// @notice Claim execution (pausable)
    /// @dev Add `whenNotPaused` to prevent claims during emergency
    // function claimExecution(...) external whenNotPaused { ... }

    /// @notice Submit proof (NOT pausable)
    /// @dev Proof submission should work even when paused to allow
    ///      provers to complete in-flight work and get paid
    // function submitProof(...) external { ... } // No modifier!

    /// @notice Cancel execution (NOT pausable)
    /// @dev Cancellation should work even when paused to allow refunds
    // function cancelExecution(...) external { ... } // No modifier!
}
