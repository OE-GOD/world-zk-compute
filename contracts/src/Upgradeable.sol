// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

/// @title ERC1967 Storage Slots
/// @notice Standard storage slots for proxy contracts
library StorageSlot {
    /// @dev Storage slot for the implementation address
    /// bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1)
    bytes32 internal constant IMPLEMENTATION_SLOT =
        0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    /// @dev Storage slot for the admin address
    /// bytes32(uint256(keccak256('eip1967.proxy.admin')) - 1)
    bytes32 internal constant ADMIN_SLOT =
        0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;

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
            case 0 {
                revert(0, returndatasize())
            }
            default {
                return(0, returndatasize())
            }
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

    // ========================================================================
    // ERRORS
    // ========================================================================

    error NotAdmin();
    error InvalidImplementation();
    error AlreadyInitialized();
    error NotInitializing();

    // ========================================================================
    // STATE
    // ========================================================================

    /// @dev Initialization state
    uint8 private _initialized;
    bool private _initializing;

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

    // ========================================================================
    // UPGRADE FUNCTIONS
    // ========================================================================

    /// @notice Upgrade to a new implementation
    /// @param newImplementation Address of new implementation
    /// @dev Override _authorizeUpgrade to add access control
    function upgradeTo(address newImplementation) external onlyAdmin {
        _authorizeUpgrade(newImplementation);
        _upgradeToAndCall(newImplementation, new bytes(0));
    }

    /// @notice Upgrade to new implementation and call function
    /// @param newImplementation Address of new implementation
    /// @param data Calldata for post-upgrade call
    function upgradeToAndCall(address newImplementation, bytes memory data) external payable onlyAdmin {
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
    // STORAGE (must maintain layout across upgrades!)
    // ========================================================================

    /// @notice Version of this implementation
    uint256 public constant VERSION = 1;

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

    // Request storage
    struct ExecutionRequest {
        uint256 id;
        address requester;
        bytes32 imageId;
        bytes32 inputDigest;
        string inputUrl;
        address callbackContract;
        uint256 tip;
        uint256 createdAt;
        uint256 expiresAt;
        uint8 status; // 0=Pending, 1=Claimed, 2=Completed, 3=Expired, 4=Cancelled
        address claimedBy;
        uint256 claimDeadline;
    }

    mapping(uint256 => ExecutionRequest) public requests;
    mapping(address => uint256) public proverCompletedCount;
    mapping(address => uint256) public proverEarnings;

    // Gap for future storage variables (upgrade safety)
    uint256[50] private __gap;

    // ========================================================================
    // EVENTS
    // ========================================================================

    event ExecutionRequested(
        uint256 indexed requestId,
        address indexed requester,
        bytes32 indexed imageId,
        uint256 tip
    );
    event ExecutionClaimed(uint256 indexed requestId, address indexed prover);
    event ExecutionCompleted(uint256 indexed requestId, address indexed prover, uint256 payout);
    event Initialized(uint256 version);

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// @notice Initialize the contract (called once via proxy)
    /// @param _registry Program registry address
    /// @param _verifier RISC Zero verifier address
    /// @param _feeRecipient Fee recipient address
    /// @param _admin Admin address for upgrades
    function initialize(
        address _registry,
        address _verifier,
        address _feeRecipient,
        address _admin
    ) external initializer {
        require(_registry != address(0), "Invalid registry");
        require(_verifier != address(0), "Invalid verifier");
        require(_feeRecipient != address(0), "Invalid fee recipient");
        require(_admin != address(0), "Invalid admin");

        registry = _registry;
        verifier = _verifier;
        feeRecipient = _feeRecipient;
        protocolFeeBps = 250; // 2.5%
        nextRequestId = 1;

        _setAdmin(_admin);

        emit Initialized(VERSION);
    }

    /// @notice Re-initialize for upgrades (bump version)
    /// @dev Called when upgrading to add new initialization logic
    function reinitialize(uint256 version) external onlyAdmin {
        // Add any new initialization logic here
        emit Initialized(version);
    }

    // ========================================================================
    // UPGRADE AUTHORIZATION
    // ========================================================================

    /// @notice Authorize upgrade (admin only)
    function _authorizeUpgrade(address newImplementation) internal override onlyAdmin {
        // Could add additional checks here:
        // - Timelock
        // - Multi-sig requirement
        // - Version validation
    }

    // ========================================================================
    // CORE FUNCTIONS (same as non-upgradeable version)
    // ========================================================================

    /// @notice Request execution
    function requestExecution(
        bytes32 imageId,
        bytes32 inputDigest,
        string calldata inputUrl,
        address callbackContract,
        uint256 expirationSeconds
    ) external payable returns (uint256 requestId) {
        require(msg.value >= 0.0001 ether, "Insufficient tip");

        requestId = nextRequestId++;

        requests[requestId] = ExecutionRequest({
            id: requestId,
            requester: msg.sender,
            imageId: imageId,
            inputDigest: inputDigest,
            inputUrl: inputUrl,
            callbackContract: callbackContract,
            tip: msg.value,
            createdAt: block.timestamp,
            expiresAt: block.timestamp + (expirationSeconds > 0 ? expirationSeconds : 1 hours),
            status: 0, // Pending
            claimedBy: address(0),
            claimDeadline: 0
        });

        emit ExecutionRequested(requestId, msg.sender, imageId, msg.value);
    }

    /// @notice Claim execution
    function claimExecution(uint256 requestId) external {
        ExecutionRequest storage req = requests[requestId];
        require(req.id != 0, "Not found");
        require(req.status == 0, "Not pending");
        require(block.timestamp <= req.expiresAt, "Expired");

        req.status = 1; // Claimed
        req.claimedBy = msg.sender;
        req.claimDeadline = block.timestamp + 10 minutes;

        emit ExecutionClaimed(requestId, msg.sender);
    }

    /// @notice Submit proof
    function submitProof(
        uint256 requestId,
        bytes calldata seal,
        bytes calldata journal
    ) external {
        ExecutionRequest storage req = requests[requestId];
        require(req.id != 0, "Not found");
        require(req.status == 1, "Not claimed");
        require(req.claimedBy == msg.sender, "Not claimant");
        require(block.timestamp <= req.claimDeadline, "Deadline passed");

        // Verify proof (simplified - would call verifier in production)
        // verifier.verify(seal, req.imageId, sha256(journal));

        req.status = 2; // Completed

        // Calculate payout
        uint256 fee = (req.tip * protocolFeeBps) / 10000;
        uint256 payout = req.tip - fee;

        // Update stats
        proverCompletedCount[msg.sender]++;
        proverEarnings[msg.sender] += payout;

        // Pay prover
        payable(msg.sender).transfer(payout);

        // Pay protocol fee
        if (fee > 0) {
            payable(feeRecipient).transfer(fee);
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
    function setProtocolFee(uint256 _feeBps) external onlyAdmin {
        require(_feeBps <= 1000, "Fee too high");
        protocolFeeBps = _feeBps;
    }

    /// @notice Set fee recipient
    function setFeeRecipient(address _recipient) external onlyAdmin {
        require(_recipient != address(0), "Invalid recipient");
        feeRecipient = _recipient;
    }
}
