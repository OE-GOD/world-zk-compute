// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @title IProverRegistry -- Interface for the decentralized prover network
/// @notice Defines the public API for prover registration, staking, reputation,
///         slashing, and selection in the World ZK Compute network.
interface IProverRegistry {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice Prover metadata and staking information
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

    /// @notice Record of a slashing event
    struct SlashEvent {
        address prover;
        uint256 amount;
        string reason;
        uint256 timestamp;
    }

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when a new prover registers with initial stake
    event ProverRegistered(address indexed prover, uint256 stake, string endpoint);

    /// @notice Emitted when a prover is deactivated
    event ProverDeactivated(address indexed prover);

    /// @notice Emitted when a prover is reactivated
    event ProverReactivated(address indexed prover);

    /// @notice Emitted when additional stake is added
    event StakeAdded(address indexed prover, uint256 amount, uint256 newTotal);

    /// @notice Emitted when stake is withdrawn
    event StakeWithdrawn(address indexed prover, uint256 amount, uint256 newTotal);

    /// @notice Emitted when a prover is slashed for misbehavior
    event ProverSlashed(address indexed prover, uint256 amount, string reason);

    /// @notice Emitted when a prover's reputation score changes
    event ReputationUpdated(address indexed prover, uint256 oldRep, uint256 newRep);

    /// @notice Emitted when a reward is distributed to a prover
    event RewardDistributed(address indexed prover, uint256 amount);

    /// @notice Emitted when a slasher's authorization status changes
    event SlasherUpdated(address indexed slasher, bool authorized);

    /// @notice Emitted when the minimum stake requirement is updated
    event MinStakeUpdated(uint256 oldMinStake, uint256 newMinStake);

    /// @notice Emitted when the slash basis points are updated
    event SlashBasisPointsUpdated(uint256 oldBasisPoints, uint256 newBasisPoints);

    // ========================================================================
    // ERRORS
    // ========================================================================

    /// @notice Thrown when stake amount is below the minimum required
    error InsufficientStake();

    /// @notice Thrown when the prover is not registered
    error ProverNotRegistered();

    /// @notice Thrown when attempting to register an already-registered prover
    error ProverAlreadyRegistered();

    /// @notice Thrown when the prover is not currently active
    error ProverNotActive();

    /// @notice Thrown when a non-authorized address attempts to slash
    error UnauthorizedSlasher();

    /// @notice Thrown when withdrawal would reduce stake below the minimum for active provers
    error WithdrawalWouldBreachMinimum();

    /// @notice Thrown when attempting to withdraw with zero stake
    error NoStakeToWithdraw();

    /// @notice Thrown when slash basis points exceed the 50% maximum
    error SlashBasisPointsTooHigh();

    /// @notice Thrown when requesting more top provers than MAX_TOP_PROVERS
    error TooManyTopProversRequested();

    // ========================================================================
    // EXTERNAL FUNCTIONS
    // ========================================================================

    /// @notice Register as a prover with initial stake
    /// @param stake Amount to stake
    /// @param endpoint P2P endpoint for coordination (optional)
    function register(uint256 stake, string calldata endpoint) external;

    /// @notice Add more stake
    /// @param amount Amount to add
    function addStake(uint256 amount) external;

    /// @notice Withdraw stake (must maintain minimum if active)
    /// @param amount Amount to withdraw
    function withdrawStake(uint256 amount) external;

    /// @notice Deactivate (stop receiving jobs, can withdraw below minimum)
    function deactivate() external;

    /// @notice Reactivate (must have minimum stake)
    function reactivate() external;

    /// @notice Slash a prover for misbehavior
    /// @param prover Address to slash
    /// @param reason Reason for slashing
    function slash(address prover, string calldata reason) external;

    /// @notice Record successful proof and update reputation
    /// @param prover Prover address
    /// @param reward Reward amount earned
    function recordSuccess(address prover, uint256 reward) external;

    /// @notice Get a weighted random prover based on stake and reputation
    /// @param seed Random seed (e.g., blockhash)
    /// @return Selected prover address
    function selectProver(uint256 seed) external view returns (address);

    /// @notice Get top N provers by reputation
    /// @param n Number of provers to return
    /// @return Top prover addresses
    function getTopProvers(uint256 n) external view returns (address[] memory);

    /// @notice Get prover info
    /// @param prover The prover address to query
    /// @return The full Prover struct
    function getProver(address prover) external view returns (Prover memory);

    /// @notice Get number of active provers
    /// @return The count of currently active provers
    function activeProverCount() external view returns (uint256);

    /// @notice Get all active prover addresses
    /// @return Array of active prover addresses
    function getActiveProvers() external view returns (address[] memory);

    /// @notice Check if address is a registered prover
    /// @param addr The address to check
    /// @return True if the address is registered
    function isProver(address addr) external view returns (bool);

    /// @notice Check if prover is active
    /// @param addr The address to check
    /// @return True if the prover is active
    function isActive(address addr) external view returns (bool);

    /// @notice Get prover's effective weight (stake * reputation)
    /// @param prover The prover address to query
    /// @return The effective weight value
    function getWeight(address prover) external view returns (uint256);

    /// @notice Update minimum stake requirement (admin only)
    /// @param _minStake New minimum stake amount
    function setMinStake(uint256 _minStake) external;

    /// @notice Update slash percentage (admin only)
    /// @param _slashBasisPoints New slash percentage in basis points (max 5000 = 50%)
    function setSlashBasisPoints(uint256 _slashBasisPoints) external;

    /// @notice Authorize/deauthorize a slasher (admin only)
    /// @param slasher Address to update
    /// @param authorized Whether the address is authorized to slash
    function setSlasher(address slasher, bool authorized) external;
}
