// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {IRiscZeroVerifier} from "risc0-ethereum/IRiscZeroVerifier.sol";
import {IProgramRegistry} from "./IProgramRegistry.sol";

/// @title ERC1967 Storage Slots
/// @notice Standard storage slots for proxy contracts
library StorageSlot {
    /// @dev Storage slot for the implementation address
    /// bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1)
    bytes32 internal constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    /// @dev Storage slot for the admin address
    /// bytes32(uint256(keccak256('eip1967.proxy.admin')) - 1)
    bytes32 internal constant ADMIN_SLOT = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;

    struct AddressSlot {
        address value;
    }

    function getAddressSlot(bytes32 slot) internal pure returns (AddressSlot storage r) {
        assembly {
            r.slot := slot
        }
    }
}

/// @title UUPS Proxy
/// @notice Minimal proxy that delegates all calls to an implementation
/// @dev Uses EIP-1967 storage slots for upgrade safety
contract UUPSProxy {
    /// @notice Deploy proxy pointing to implementation
    /// @param implementation Address of the implementation contract
    /// @param data Initialization calldata
    constructor(address implementation, bytes memory data) {
        StorageSlot.getAddressSlot(StorageSlot.IMPLEMENTATION_SLOT).value = implementation;

        if (data.length > 0) {
            (bool success, bytes memory returndata) = implementation.delegatecall(data);
            if (!success) {
                if (returndata.length > 0) {
                    assembly {
                        revert(add(32, returndata), mload(returndata))
                    }
                } else {
                    revert("Initialization failed");
                }
            }
        }
    }

    /// @notice Fallback function delegates all calls to implementation
    fallback() external payable {
        address implementation = StorageSlot.getAddressSlot(StorageSlot.IMPLEMENTATION_SLOT).value;

        assembly {
            calldatacopy(0, 0, calldatasize())
            let result := delegatecall(gas(), implementation, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())

            switch result
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
        }
    }

    receive() external payable {}
}

/// @title UUPS Upgradeable Base
/// @notice Base contract for UUPS upgradeable implementations
/// @dev Inherit this in your implementation contracts
abstract contract UUPSUpgradeable {
    // ========================================================================
    // EVENTS
    // ========================================================================

    event Upgraded(address indexed implementation);
    event AdminChanged(address previousAdmin, address newAdmin);
    event TimelockChanged(address previousTimelock, address newTimelock);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error NotAdmin();
    error NotTimelocked();
    error InvalidImplementation();
    error AlreadyInitialized();
    error NotInitializing();

    // ========================================================================
    // STATE
    // ========================================================================

    /// @dev Initialization state
    uint8 internal _initialized;
    bool private _initializing;

    /// @notice Optional timelock controller address for critical operations
    /// @dev When set, functions guarded by onlyTimelocked require msg.sender == timelock.
    ///      When not set (address(0)), those functions fall back to requiring msg.sender == admin.
    address public timelock;

    // ========================================================================
    // MODIFIERS
    // ========================================================================

    /// @notice Restrict to admin only
    modifier onlyAdmin() {
        if (msg.sender != _getAdmin()) revert NotAdmin();
        _;
    }

    /// @notice Ensure contract is being initialized
    modifier initializer() {
        bool isTopLevelCall = !_initializing;
        if ((!isTopLevelCall || _initialized >= 1) && (address(this).code.length > 0 || _initialized != 1)) {
            revert AlreadyInitialized();
        }
        _initialized = 1;
        if (isTopLevelCall) {
            _initializing = true;
        }
        _;
        if (isTopLevelCall) {
            _initializing = false;
        }
    }

    /// @notice Ensure only called during initialization
    modifier onlyInitializing() {
        if (!_initializing) revert NotInitializing();
        _;
    }

    /// @notice Restrict to timelock if one is set, otherwise to admin
    /// @dev When timelock == address(0), falls back to admin-only (backwards compatible).
    ///      When timelock is set, requires msg.sender == timelock (i.e., the timelock
    ///      controller must be the caller after its delay has passed).
    modifier onlyTimelocked() {
        if (timelock != address(0)) {
            if (msg.sender != timelock) revert NotTimelocked();
        } else {
            if (msg.sender != _getAdmin()) revert NotAdmin();
        }
        _;
    }

    // ========================================================================
    // UPGRADE FUNCTIONS
    // ========================================================================

    /// @notice Upgrade to a new implementation
    /// @param newImplementation Address of new implementation
    /// @dev Override _authorizeUpgrade to add access control.
    ///      Uses onlyTimelocked: if a timelock is set, only the timelock can call this.
    function upgradeTo(address newImplementation) external onlyTimelocked {
        _authorizeUpgrade(newImplementation);
        _upgradeToAndCall(newImplementation, new bytes(0));
    }

    /// @notice Upgrade to new implementation and call function
    /// @param newImplementation Address of new implementation
    /// @param data Calldata for post-upgrade call
    /// @dev Uses onlyTimelocked: if a timelock is set, only the timelock can call this.
    function upgradeToAndCall(address newImplementation, bytes memory data) external payable onlyTimelocked {
        _authorizeUpgrade(newImplementation);
        _upgradeToAndCall(newImplementation, data);
    }

    /// @notice Authorization hook for upgrades
    /// @dev Override this to add custom authorization logic
    function _authorizeUpgrade(address newImplementation) internal virtual;

    /// @notice Internal upgrade function
    function _upgradeToAndCall(address newImplementation, bytes memory data) internal {
        // Validate new implementation
        if (newImplementation.code.length == 0) revert InvalidImplementation();

        // Store new implementation
        StorageSlot.getAddressSlot(StorageSlot.IMPLEMENTATION_SLOT).value = newImplementation;

        emit Upgraded(newImplementation);

        // Call initialization function if data provided
        if (data.length > 0) {
            (bool success, bytes memory returndata) = newImplementation.delegatecall(data);
            if (!success) {
                if (returndata.length > 0) {
                    assembly {
                        revert(add(32, returndata), mload(returndata))
                    }
                } else {
                    revert("Upgrade call failed");
                }
            }
        }
    }

    // ========================================================================
    // ADMIN FUNCTIONS
    // ========================================================================

    /// @notice Get current admin
    function admin() external view returns (address) {
        return _getAdmin();
    }

    /// @notice Change admin
    function changeAdmin(address newAdmin) external onlyAdmin {
        require(newAdmin != address(0), "Invalid admin");
        emit AdminChanged(_getAdmin(), newAdmin);
        StorageSlot.getAddressSlot(StorageSlot.ADMIN_SLOT).value = newAdmin;
    }

    /// @notice Set or clear the timelock controller address
    /// @param _timelock The timelock controller address, or address(0) to disable
    /// @dev Only the admin can set the timelock. Once set, critical operations
    ///      guarded by onlyTimelocked will require calls through the timelock.
    function setTimelock(address _timelock) external onlyAdmin {
        address oldTimelock = timelock;
        timelock = _timelock;
        emit TimelockChanged(oldTimelock, _timelock);
    }

    /// @notice Get admin from storage
    function _getAdmin() internal view returns (address) {
        return StorageSlot.getAddressSlot(StorageSlot.ADMIN_SLOT).value;
    }

    /// @notice Set admin (only during initialization)
    function _setAdmin(address newAdmin) internal {
        StorageSlot.getAddressSlot(StorageSlot.ADMIN_SLOT).value = newAdmin;
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get current implementation address
    function implementation() external view returns (address) {
        return StorageSlot.getAddressSlot(StorageSlot.IMPLEMENTATION_SLOT).value;
    }

    /// @notice Get proxy version
    function proxiableUUID() external pure returns (bytes32) {
        return StorageSlot.IMPLEMENTATION_SLOT;
    }
}

/// @title UpgradeableExecutionEngine
/// @notice Upgradeable version of ExecutionEngine
/// @dev Demonstrates how to make ExecutionEngine upgradeable
contract UpgradeableExecutionEngine is UUPSUpgradeable {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Version of this implementation
    uint256 public constant VERSION = 1;

    /// @notice Default protocol fee in basis points (2.5%)
    uint256 private constant DEFAULT_PROTOCOL_FEE_BPS = 250;

    /// @notice Maximum protocol fee in basis points (10%)
    uint256 private constant MAX_FEE_BPS = 1000;

    /// @notice Default expiration time for requests when none specified
    uint256 public constant DEFAULT_EXPIRATION = 1 hours;

    /// @notice Maximum allowed expiration duration (30 days)
    uint256 public constant MAX_EXPIRATION = 30 days;

    /// @notice Minimum allowed expiration duration (1 minute)
    uint256 public constant MIN_EXPIRATION = 1 minutes;

    // ========================================================================
    // STORAGE (must maintain layout across upgrades!)
    // ========================================================================

    /// @notice The program registry
    address public registry;

    /// @notice The RISC Zero verifier contract
    address public verifier;

    /// @notice Protocol fee (basis points)
    uint256 public protocolFeeBps;

    /// @notice Fee recipient
    address public feeRecipient;

    /// @notice Next request ID
    uint256 public nextRequestId;

    // Request storage (packed for gas efficiency)
    struct ExecutionRequest {
        uint256 id; // Slot 0
        bytes32 imageId; // Slot 1
        bytes32 inputDigest; // Slot 2
        address requester; // Slot 3: requester(20) + createdAt(6) + expiresAt(6)
        uint48 createdAt;
        uint48 expiresAt;
        address callbackContract; // Slot 4: callback(20) + status(1)
        uint8 status; // 0=Pending, 1=Claimed, 2=Completed, 3=Expired, 4=Cancelled
        address claimedBy; // Slot 5: claimedBy(20) + claimDeadline(6)
        uint48 claimDeadline;
        uint256 tip; // Slot 6
    }

    mapping(uint256 => ExecutionRequest) public requests;
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    /// @notice Whether the contract is paused
    bool public paused;

    /// @dev Reentrancy guard status (1 = not entered, 2 = entered)
    uint256 private _reentrancyStatus;

    // Gap for future storage variables (upgrade safety)
    // Reduced from 50 to 48 to account for 2 new storage slots above
    uint256[48] private __gap;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        string inputUrl,
        uint8 inputType,
        uint256 tip
    );
    event ExecutionClaimed(uint256 indexed requestId, address indexed prover);
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, uint256 payout);
    event Initialized(uint256 version);
    event ProtocolFeeUpdated(uint256 oldFeeBps, uint256 newFeeBps);
    event FeeRecipientUpdated(address indexed oldRecipient, address indexed newRecipient);
    event Paused(address account);
    event Unpaused(address account);

    // ========================================================================
    // ERRORS
    // ========================================================================

    error EnforcedPause();
    error ExpectedPause();
    error ReentrancyGuardReentrantCall();
    error TransferFailed();
    error FeeTooHigh();
    error ZeroImageId();
    error ProgramNotActive();
    error ExpirationTooLong();
    error ExpirationTooShort();
    error ExpirationOverflow();

    // ========================================================================
    // MODIFIERS
    // ========================================================================

    /// @notice Restrict function calls when the contract is paused
    modifier whenNotPaused() {
        if (paused) revert EnforcedPause();
        _;
    }

    /// @notice Prevent reentrant calls
    modifier nonReentrant() {
        if (_reentrancyStatus == 2) revert ReentrancyGuardReentrantCall();
        _reentrancyStatus = 2;
        _;
        _reentrancyStatus = 1;
    }

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// @notice Initialize the contract (called once via proxy)
    /// @param _registry Program registry address
    /// @param _verifier RISC Zero verifier address
    /// @param _feeRecipient Fee recipient address
    /// @param _admin Admin address for upgrades
    function initialize(address _registry, address _verifier, address _feeRecipient, address _admin)
        external
        initializer
    {
        require(_registry != address(0), "Invalid registry");
        require(_verifier != address(0), "Invalid verifier");
        require(_feeRecipient != address(0), "Invalid fee recipient");
        require(_admin != address(0), "Invalid admin");

        registry = _registry;
        verifier = _verifier;
        feeRecipient = _feeRecipient;
        protocolFeeBps = DEFAULT_PROTOCOL_FEE_BPS;
        nextRequestId = 1;
        _reentrancyStatus = 1; // Initialize reentrancy guard

        _setAdmin(_admin);

        emit Initialized(VERSION);
    }

    /// @notice Re-initialize for upgrades (bump version)
    /// @dev Called when upgrading to add new initialization logic.
    ///      Version must be strictly greater than the current initialized version.
    function reinitialize(uint8 version) external onlyAdmin {
        if (version <= _initialized) revert AlreadyInitialized();
        _initialized = version;
        emit Initialized(version);
    }

    // ========================================================================
    // UPGRADE AUTHORIZATION
    // ========================================================================

    /// @notice Authorize upgrade
    /// @dev Access control is enforced by the onlyTimelocked modifier on upgradeTo/upgradeToAndCall.
    function _authorizeUpgrade(address newImplementation) internal override {
        if (newImplementation.code.length == 0) revert InvalidImplementation();
    }

    // ========================================================================
    // CORE FUNCTIONS (same as non-upgradeable version)
    // ========================================================================

    /// @notice Request execution with inputType
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds,
        uint8 inputType
    ) external payable returns (uint256 requestId) {
        if (imageId == bytes32(0)) revert ZeroImageId();
        require(msg.value >= 0.0001 ether, "Insufficient tip");
        if (!IProgramRegistry(registry).isProgramActive(imageId)) revert ProgramNotActive();

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : DEFAULT_EXPIRATION;
        if (expiration > MAX_EXPIRATION) revert ExpirationTooLong();
        if (expiration < MIN_EXPIRATION) revert ExpirationTooShort();
        if (block.timestamp + expiration > type(uint48).max) revert ExpirationOverflow();

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            // forge-lint: disable-next-line(unsafe-typecast)
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: 0, // Pending
            claimedBy: address(0),
            claimDeadline: 0,
            tip: msg.value
        });

        emit ExecutionRequested(requestId, msg.sender, imageId, inputUrl, inputType, msg.value);
    }

    /// @notice Request execution (backward-compatible, defaults to public input)
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable returns (uint256 requestId) {
        if (imageId == bytes32(0)) revert ZeroImageId();
        require(msg.value >= 0.0001 ether, "Insufficient tip");
        if (!IProgramRegistry(registry).isProgramActive(imageId)) revert ProgramNotActive();

        uint256 expiration = expirationSeconds > 0 ? expirationSeconds : DEFAULT_EXPIRATION;
        if (expiration > MAX_EXPIRATION) revert ExpirationTooLong();
        if (expiration < MIN_EXPIRATION) revert ExpirationTooShort();
        if (block.timestamp + expiration > type(uint48).max) revert ExpirationOverflow();

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            imageId: imageId,
            inputDigest: inputDigest,
            requester: msg.sender,
            createdAt: uint48(block.timestamp),
            // forge-lint: disable-next-line(unsafe-typecast)
            expiresAt: uint48(block.timestamp + expiration),
            callbackContract: callbackContract,
            status: 0, // Pending
            claimedBy: address(0),
            claimDeadline: 0,
            tip: msg.value
        });

        emit ExecutionRequested(requestId, msg.sender, imageId, inputUrl, 0, msg.value);
    }

    /// @notice Claim execution
    function claimExecution(uint256 requestId) external whenNotPaused {
        ExecutionRequest storage req = requests[requestId];
        require(req.id != 0, "Not found");
        require(req.status == 0, "Not pending");
        require(block.timestamp <= req.expiresAt, "Expired");

        req.status = 1; // Claimed
        req.claimedBy = msg.sender;
        // forge-lint: disable-next-line(unsafe-typecast)
        req.claimDeadline = uint48(block.timestamp + 10 minutes);

        emit ExecutionClaimed(requestId, msg.sender);
    }

    /// @notice Submit proof
    function submitProof(uint256 requestId, bytes calldata seal, bytes calldata journal)
        external
        nonReentrant
        whenNotPaused
    {
        ExecutionRequest storage req = requests[requestId];
        require(req.id != 0, "Not found");
        require(req.status == 1, "Not claimed");
        require(req.claimedBy == msg.sender, "Not claimant");
        require(block.timestamp <= req.claimDeadline, "Deadline passed");
        require(seal.length > 0, "Empty seal");
        require(journal.length > 0, "Empty journal");

        // Verify the proof - reverts if invalid
        bytes32 journalDigest = sha256(journal);
        IRiscZeroVerifier(verifier).verify(seal, req.imageId, journalDigest);

        req.status = 2; // Completed

        // Calculate payout
        uint256 fee = (req.tip * protocolFeeBps) / 10000;
        uint256 payout = req.tip - fee;

        // Update stats
        proverCompletedCount[msg.sender]++;
        proverEarnings[msg.sender] += payout;

        // Pay prover (using call instead of transfer for contract wallet compatibility)
        (bool proverPaid,) = payable(msg.sender).call{value: payout}("");
        if (!proverPaid) revert TransferFailed();

        // Pay protocol fee
        if (fee > 0) {
            (bool feePaid,) = payable(feeRecipient).call{value: fee}("");
            if (!feePaid) revert TransferFailed();
        }

        emit ExecutionCompleted(requestId, msg.sender, payout);
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get request
    function getRequest(uint256 requestId) external view returns (ExecutionRequest memory) {
        return requests[requestId];
    }

    /// @notice Get prover stats
    function getProverStats(address prover) external view returns (uint256 completed, uint256 earnings) {
        return (proverCompletedCount[prover], proverEarnings[prover]);
    }

    // ========================================================================
    // ADMIN FUNCTIONS
    // ========================================================================

    /// @notice Set protocol fee
    /// @dev Uses onlyTimelocked: if a timelock is set, only the timelock can call this.
    function setProtocolFee(uint256 _feeBps) external onlyTimelocked {
        if (_feeBps > MAX_FEE_BPS) revert FeeTooHigh();
        uint256 oldFeeBps = protocolFeeBps;
        protocolFeeBps = _feeBps;
        emit ProtocolFeeUpdated(oldFeeBps, _feeBps);
    }

    /// @notice Set fee recipient
    /// @dev Uses onlyTimelocked: if a timelock is set, only the timelock can call this.
    function setFeeRecipient(address _recipient) external onlyTimelocked {
        require(_recipient != address(0), "Invalid recipient");
        address oldRecipient = feeRecipient;
        feeRecipient = _recipient;
        emit FeeRecipientUpdated(oldRecipient, _recipient);
    }

    /// @notice Pause the contract, blocking claims and proof submissions
    function pause() external onlyAdmin {
        if (paused) revert EnforcedPause();
        paused = true;
        emit Paused(msg.sender);
    }

    /// @notice Unpause the contract
    function unpause() external onlyAdmin {
        if (!paused) revert ExpectedPause();
        paused = false;
        emit Unpaused(msg.sender);
    }
}
