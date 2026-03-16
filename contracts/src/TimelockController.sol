// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title TimelockController
/// @notice Minimal timelock for high-impact admin operations.
/// @dev Operations are scheduled with a delay, then executed after the delay has passed.
///      Only the owner can schedule and cancel operations. Anyone can execute once the
///      delay has elapsed. This prevents instant rug-pulls by requiring a public waiting
///      period before critical configuration changes take effect.
contract TimelockController {
    // ========================================================================
    // TYPES
    // ========================================================================

    /// @notice State of a scheduled operation
    struct Operation {
        address target;
        bytes data;
        uint256 readyTimestamp;
        bool executed;
        bool cancelled;
    }

    // ========================================================================
    // STATE
    // ========================================================================

    /// @notice The contract owner who can schedule and cancel operations
    address public owner;

    /// @notice Minimum delay in seconds before a scheduled operation can be executed
    uint256 public minDelay;

    /// @notice Mapping from operation ID to its details
    mapping(bytes32 => Operation) private _operations;

    // ========================================================================
    // EVENTS
    // ========================================================================

    /// @notice Emitted when an operation is scheduled
    event OperationScheduled(
        bytes32 indexed id, address indexed target, bytes data, uint256 delay, uint256 readyTimestamp
    );

    /// @notice Emitted when a scheduled operation is executed
    event OperationExecuted(bytes32 indexed id);

    /// @notice Emitted when a pending operation is cancelled
    event OperationCancelled(bytes32 indexed id);

    /// @notice Emitted when the minimum delay is updated
    event MinDelayUpdated(uint256 oldDelay, uint256 newDelay);

    /// @notice Emitted when ownership is transferred
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error NotOwner();
    error OperationAlreadyScheduled();
    error OperationNotScheduled();
    error OperationNotReady();
    error OperationAlreadyExecuted();
    error OperationCancelledError();
    error DelayBelowMinimum();
    error ExecutionFailed();
    error ZeroAddress();
    error ZeroDelay();

    // ========================================================================
    // MODIFIERS
    // ========================================================================

    /// @notice Restrict to owner only
    modifier onlyOwner() {
        if (msg.sender != owner) revert NotOwner();
        _;
    }

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @notice Deploy the timelock controller
    /// @param _minDelay Minimum delay in seconds (e.g., 172800 for 48 hours)
    /// @param _owner The initial owner who can schedule and cancel operations
    constructor(uint256 _minDelay, address _owner) {
        if (_minDelay == 0) revert ZeroDelay();
        if (_owner == address(0)) revert ZeroAddress();

        minDelay = _minDelay;
        owner = _owner;

        emit OwnershipTransferred(address(0), _owner);
    }

    // ========================================================================
    // SCHEDULE / EXECUTE / CANCEL
    // ========================================================================

    /// @notice Schedule an operation for future execution
    /// @param id Unique identifier for this operation (typically a descriptive hash)
    /// @param target The contract to call when executing
    /// @param data The calldata to pass to the target
    /// @param delay The delay in seconds before the operation can be executed (must be >= minDelay)
    function schedule(bytes32 id, address target, bytes calldata data, uint256 delay) external onlyOwner {
        if (delay < minDelay) revert DelayBelowMinimum();
        if (_operations[id].readyTimestamp != 0) revert OperationAlreadyScheduled();
        if (target == address(0)) revert ZeroAddress();

        uint256 readyTimestamp = block.timestamp + delay;

        _operations[id] = Operation({
            target: target,
            data: data,
            readyTimestamp: readyTimestamp,
            executed: false,
            cancelled: false
        });

        emit OperationScheduled(id, target, data, delay, readyTimestamp);
    }

    /// @notice Execute a scheduled operation after its delay has passed
    /// @param id The operation identifier
    /// @dev Anyone can call this once the delay has passed. This ensures operations
    ///      cannot be blocked by an unresponsive owner.
    function execute(bytes32 id) external {
        Operation storage op = _operations[id];

        if (op.readyTimestamp == 0) revert OperationNotScheduled();
        if (op.executed) revert OperationAlreadyExecuted();
        if (op.cancelled) revert OperationCancelledError();
        if (block.timestamp < op.readyTimestamp) revert OperationNotReady();

        op.executed = true;

        (bool success, bytes memory returndata) = op.target.call(op.data);
        if (!success) {
            if (returndata.length > 0) {
                assembly {
                    revert(add(32, returndata), mload(returndata))
                }
            } else {
                revert ExecutionFailed();
            }
        }

        emit OperationExecuted(id);
    }

    /// @notice Cancel a pending (not yet executed) operation
    /// @param id The operation identifier
    function cancel(bytes32 id) external onlyOwner {
        Operation storage op = _operations[id];

        if (op.readyTimestamp == 0) revert OperationNotScheduled();
        if (op.executed) revert OperationAlreadyExecuted();
        if (op.cancelled) revert OperationCancelledError();

        op.cancelled = true;

        emit OperationCancelled(id);
    }

    // ========================================================================
    // ADMIN
    // ========================================================================

    /// @notice Update the minimum delay for future operations
    /// @param _minDelay New minimum delay in seconds
    /// @dev This itself should ideally be timelocked in production. For simplicity,
    ///      it is owner-only here.
    function setMinDelay(uint256 _minDelay) external onlyOwner {
        if (_minDelay == 0) revert ZeroDelay();
        uint256 oldDelay = minDelay;
        minDelay = _minDelay;
        emit MinDelayUpdated(oldDelay, _minDelay);
    }

    /// @notice Transfer ownership to a new address
    /// @param newOwner The new owner address
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        address oldOwner = owner;
        owner = newOwner;
        emit OwnershipTransferred(oldOwner, newOwner);
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get the details of a scheduled operation
    /// @param id The operation identifier
    /// @return target The target contract address
    /// @return data The calldata
    /// @return readyTimestamp When the operation can be executed (0 if not scheduled)
    /// @return executed Whether the operation has been executed
    /// @return cancelled Whether the operation has been cancelled
    function getOperation(bytes32 id)
        external
        view
        returns (address target, bytes memory data, uint256 readyTimestamp, bool executed, bool cancelled)
    {
        Operation storage op = _operations[id];
        return (op.target, op.data, op.readyTimestamp, op.executed, op.cancelled);
    }

    /// @notice Check if an operation is pending (scheduled but not executed or cancelled)
    /// @param id The operation identifier
    /// @return True if the operation is pending
    function isOperationPending(bytes32 id) external view returns (bool) {
        Operation storage op = _operations[id];
        return op.readyTimestamp != 0 && !op.executed && !op.cancelled;
    }

    /// @notice Check if an operation is ready to execute
    /// @param id The operation identifier
    /// @return True if the operation can be executed now
    function isOperationReady(bytes32 id) external view returns (bool) {
        Operation storage op = _operations[id];
        return op.readyTimestamp != 0 && !op.executed && !op.cancelled && block.timestamp >= op.readyTimestamp;
    }
}
