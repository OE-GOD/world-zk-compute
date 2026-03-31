// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {RemainderVerifier} from "./RemainderVerifier.sol";

/// @title RemainderVerifierUpgradeable
/// @notice Production-hardened UUPS-upgradeable RemainderVerifier with two-step admin
///         transfer, OZ-style owner role, and storage gaps.
///
/// @dev This contract extends the existing RemainderVerifier (which has custom UUPS +
///      admin-based access control) and adds:
///
///      - Constructor prevents direct initialization of implementation contract
///      - Two-step admin transfer (`transferAdmin` / `acceptAdmin`)
///      - OZ-compatible Ownable pattern using ERC-7201 namespaced storage
///      - Version tracking
///      - Storage gap for future upgrade safety
///
///      Deployment:
///      1. Deploy this contract as the implementation
///      2. Deploy UUPSProxy(implementation, abi.encodeCall(initializeV2, (admin)))
///      3. Cast the proxy address to RemainderVerifierUpgradeable
///
///      Role model:
///      - Legacy Admin (EIP-1967 slot): circuit registration, pause/unpause, deactivation
///      - OZ Owner (ERC-7201 slot): separate governance role (can be same address)
///      - Timelock: when set, guards upgrades and critical operations
///
///      Upgrade authorization flows through the parent's `onlyTimelocked` modifier on
///      `upgradeTo()` / `upgradeToAndCall()`. The `_authorizeUpgrade()` hook validates
///      implementation code exists. This is a deliberate design: the timelock provides
///      time-delayed, multi-sig compatible upgrade governance.
contract RemainderVerifierUpgradeable is RemainderVerifier {
    // ========================================================================
    // CONSTANTS
    // ========================================================================

    /// @notice Implementation version (bump on each upgrade)
    uint256 public constant VERSION = 1;

    // ========================================================================
    // OZ-STYLE OWNABLE (ERC-7201 NAMESPACED STORAGE)
    // ========================================================================

    /// @custom:storage-location erc7201:remainderverifier.storage.Ownable
    struct OwnableStorage {
        address _owner;
    }

    /// @dev keccak256(abi.encode(uint256(keccak256("remainderverifier.storage.Ownable")) - 1))
    ///      & ~bytes32(uint256(0xff))
    bytes32 private constant OWNABLE_STORAGE_LOCATION =
        0xb4a22de657aa2c4300bfd003e3f96fe42968f7e3fd142ae7d1fe0ceae0645c00;

    function _getOwnableStorage() private pure returns (OwnableStorage storage $) {
        assembly {
            $.slot := OWNABLE_STORAGE_LOCATION
        }
    }

    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    error OwnableUnauthorizedAccount(address account);
    error OwnableInvalidOwner(address ownerAddr);

    modifier onlyOwner() {
        if (owner() != msg.sender) {
            revert OwnableUnauthorizedAccount(msg.sender);
        }
        _;
    }

    /// @notice Get the current OZ-style owner
    function owner() public view returns (address) {
        return _getOwnableStorage()._owner;
    }

    function _transferOwnership(address newOwner) internal {
        OwnableStorage storage $ = _getOwnableStorage();
        address oldOwner = $._owner;
        $._owner = newOwner;
        emit OwnershipTransferred(oldOwner, newOwner);
    }

    /// @notice Transfer OZ ownership
    /// @param newOwner The new owner address
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert OwnableInvalidOwner(address(0));
        _transferOwnership(newOwner);
    }

    /// @notice Renounce OZ ownership
    /// @dev WARNING: This is irreversible.
    function renounceOwnership() external onlyOwner {
        _transferOwnership(address(0));
    }

    // ========================================================================
    // TWO-STEP ADMIN TRANSFER
    // ========================================================================

    /// @custom:storage-location erc7201:remainderverifier.storage.PendingAdmin
    struct PendingAdminStorage {
        address _pendingAdmin;
    }

    /// @dev keccak256(abi.encode(uint256(keccak256("remainderverifier.storage.PendingAdmin")) - 1))
    ///      & ~bytes32(uint256(0xff))
    bytes32 private constant PENDING_ADMIN_STORAGE_LOCATION =
        0x25b7adbb90e0423e8e0c3f1e41f9b3e30c24cc29c8697b1e230e09c5d4b2c800;

    function _getPendingAdminStorage() private pure returns (PendingAdminStorage storage $) {
        assembly {
            $.slot := PENDING_ADMIN_STORAGE_LOCATION
        }
    }

    event AdminTransferStarted(address indexed currentAdmin, address indexed newPendingAdmin);
    event AdminTransferAccepted(address indexed previousAdmin, address indexed newAdmin);
    event AdminTransferCancelled(address indexed currentAdmin);

    error PendingAdminIsZero();
    error NotPendingAdmin();

    /// @notice Get the current pending admin address
    function pendingAdmin() external view returns (address) {
        return _getPendingAdminStorage()._pendingAdmin;
    }

    /// @notice Start a two-step admin transfer (safer than single-step changeAdmin)
    /// @param newAdmin The proposed new admin address
    /// @dev Only the current admin can initiate. The new admin must call acceptAdmin().
    function transferAdmin(address newAdmin) external onlyAdmin {
        if (newAdmin == address(0)) revert PendingAdminIsZero();
        _getPendingAdminStorage()._pendingAdmin = newAdmin;
        emit AdminTransferStarted(_getAdmin(), newAdmin);
    }

    /// @notice Accept the pending admin transfer
    /// @dev Only the pending admin can call this, completing the two-step transfer.
    function acceptAdmin() external {
        PendingAdminStorage storage $ = _getPendingAdminStorage();
        if (msg.sender != $._pendingAdmin) revert NotPendingAdmin();
        address oldAdmin = _getAdmin();
        _setAdmin($._pendingAdmin);
        $._pendingAdmin = address(0);
        emit AdminTransferAccepted(oldAdmin, msg.sender);
    }

    /// @notice Cancel a pending admin transfer
    /// @dev Only the current admin can cancel.
    function cancelAdminTransfer() external onlyAdmin {
        _getPendingAdminStorage()._pendingAdmin = address(0);
        emit AdminTransferCancelled(_getAdmin());
    }

    // ========================================================================
    // STORAGE GAP
    // ========================================================================

    /// @dev Reserved storage slots for future upgrades.
    ///      When adding new state variables in V2+, reduce __gap size accordingly.
    uint256[49] private __gap;

    // ========================================================================
    // CONSTRUCTOR
    // ========================================================================

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        // Prevent the implementation contract from being initialized directly.
        // Only the proxy should be initialized via initializeV2().
        // Set to max to block any initialize or initializeV2 calls.
        _initialized = type(uint8).max;
    }

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// @notice Initialize the upgradeable verifier (called once via proxy)
    /// @param _admin Admin address for the legacy admin, OZ owner, and initial configuration
    /// @dev Uses version 2 to be forward-compatible with the parent's `initialize(address)`
    ///      which uses version 1. Call via proxy only.
    function initializeV2(address _admin) external {
        // Manual version check: require we haven't been initialized to v2+ yet
        require(_initialized < 2, "Already initialized to v2+");
        _initialized = 2;

        // Set the legacy admin (EIP-1967 slot) for circuit management / pause
        _setAdmin(_admin);

        // Set the OZ owner for governance
        _transferOwnership(_admin);
    }

    // ========================================================================
    // VIEW FUNCTIONS
    // ========================================================================

    /// @notice Get the implementation version
    function getVersion() external pure returns (uint256) {
        return VERSION;
    }

    /// @notice Get the initialization version
    function getInitializedVersion() external view returns (uint8) {
        return _initialized;
    }
}
