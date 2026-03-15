// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title IProverReputation -- Interface for prover reputation tracking
/// @notice Defines the public API for tracking prover reliability, performance scores,
///         tier classifications, and slashing on World ZK Compute.
interface IProverReputation {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Prover reputation data
    struct Reputation {
        uint64 totalJobs;
        uint64 completedJobs;
        uint64 failedJobs;
        uint64 abandonedJobs;
        uint256 totalEarnings;
        uint64 avgProofTimeMs;
        uint64 lastJobAt;
        uint64 lastUpdateAt;
        uint32 score;
        uint8 tier;
        bool isRegistered;
        bool isSlashed;
        bool isBanned;
    }

    /// @notice Reputation tier levels
    enum Tier {
        Unranked, // New prover, no history
        Bronze, // Score < 5000
        Silver, // Score 5000-7499
        Gold, // Score 7500-8999
        Platinum, // Score 9000-9499
        Diamond // Score 9500+
    }

    /// @notice Slashing event details
    struct SlashEvent {
        uint256 timestamp;
        string reason;
        uint256 penaltyBps;
        address reportedBy;
    }

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when a new prover registers
    event ProverRegistered(address indexed prover, uint256 timestamp);

    /// @notice Emitted when a prover completes a job successfully
    event JobCompleted(address indexed prover, uint256 proofTimeMs, uint256 newScore);

    /// @notice Emitted when a prover fails a job
    event JobFailed(address indexed prover, string reason, uint256 newScore);

    /// @notice Emitted when a prover abandons a job
    event JobAbandoned(address indexed prover, uint256 requestId, uint256 newScore);

    /// @notice Emitted when a prover is slashed
    event ProverSlashed(address indexed prover, string reason, uint256 penaltyBps);

    /// @notice Emitted when a prover is banned
    event ProverBanned(address indexed prover, string reason);

    /// @notice Emitted when a prover is unbanned
    event ProverUnbanned(address indexed prover);

    /// @notice Emitted when a prover's tier changes
    event TierChanged(address indexed prover, uint8 oldTier, uint8 newTier);

    /// @notice Emitted when a reporter is authorized
    event ReporterAuthorized(address indexed reporter);

    /// @notice Emitted when a reporter's authorization is revoked
    event ReporterRevoked(address indexed reporter);

    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Thrown when msg.sender is not the contract owner
    error NotOwner();

    /// @notice Thrown when msg.sender is not an authorized reporter or owner
    error NotAuthorized();

    /// @notice Thrown when the prover is not registered
    error ProverNotRegistered();

    /// @notice Thrown when the prover is banned
    error ProverIsBanned();

    /// @notice Thrown when attempting to register an already-registered prover
    error AlreadyRegistered();

    /// @notice Thrown when an invalid score value is provided
    error InvalidScore();

    // ========================================================================
    // EXTERNAL FUNCTIONS
    // ========================================================================

    /// @notice Register as a prover (sets initial score to 50%)
    function register() external;

    /// @notice Record successful job completion
    /// @param prover Prover address
    /// @param proofTimeMs Time taken to generate proof in milliseconds
    /// @param earnings Amount earned from the job
    function recordSuccess(address prover, uint256 proofTimeMs, uint256 earnings) external;

    /// @notice Record failed job
    /// @param prover Prover address
    /// @param reason Failure reason
    function recordFailure(address prover, string calldata reason) external;

    /// @notice Record abandoned job (claimed but never submitted)
    /// @param prover Prover address
    /// @param requestId Request that was abandoned
    function recordAbandon(address prover, uint256 requestId) external;

    /// @notice Slash a prover for misbehavior (owner only)
    /// @param prover Prover address
    /// @param reason Slash reason
    /// @param penaltyBps Penalty in basis points
    function slash(address prover, string calldata reason, uint256 penaltyBps) external;

    /// @notice Ban a prover (owner only)
    /// @param prover Prover address
    /// @param reason Ban reason
    function ban(address prover, string calldata reason) external;

    /// @notice Unban a prover (owner only)
    /// @param prover Prover address
    function unban(address prover) external;

    /// @notice Get prover reputation
    /// @param prover The prover address to query
    /// @return The full Reputation struct
    function getReputation(address prover) external view returns (Reputation memory);

    /// @notice Get prover score (with time-based decay applied)
    /// @param prover The prover address to query
    /// @return The current decayed score
    function getScore(address prover) external view returns (uint256);

    /// @notice Get prover tier
    /// @param prover The prover address to query
    /// @return The current tier
    function getTier(address prover) external view returns (Tier);

    /// @notice Get success rate (percentage with 2 decimal precision, e.g. 9500 = 95.00%)
    /// @param prover The prover address to query
    /// @return Success rate in basis points
    function getSuccessRate(address prover) external view returns (uint256);

    /// @notice Check if prover is in good standing (registered, not banned, score >= Silver)
    /// @param prover The prover address to check
    /// @return True if the prover is in good standing
    function isGoodStanding(address prover) external view returns (bool);

    /// @notice Get slash history for a prover
    /// @param prover The prover address to query
    /// @return Array of SlashEvent structs
    function getSlashHistory(address prover) external view returns (SlashEvent[] memory);

    /// @notice Get number of provers at a given tier
    /// @param tier The tier to query
    /// @return Count of provers in that tier
    function getProversByTier(Tier tier) external view returns (uint256);

    /// @notice Authorize a reporter (e.g., execution engine). Owner only.
    /// @param reporter Address to authorize
    function authorizeReporter(address reporter) external;

    /// @notice Revoke reporter authorization. Owner only.
    /// @param reporter Address to revoke
    function revokeReporter(address reporter) external;

    /// @notice Transfer ownership. Owner only.
    /// @param newOwner New owner address
    function transferOwnership(address newOwner) external;
}
