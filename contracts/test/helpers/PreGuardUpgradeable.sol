// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import {StorageSlot} from "../../src/Upgradeable.sol";

// The UUPS base as it was before the upgrade check (commit c12a550): it checks only that the new
// implementation has code, and proxiableUUID() is pure, so it also answers through a proxy. Every
// TEEMLVerifier and RemainderVerifier deployed before that change runs this code, so tests use it to
// stand for those implementations. Only the contract name and these lines differ from that commit.

/// @title UUPS Upgradeable Base
/// @notice Base contract for UUPS upgradeable implementations
/// @dev Inherit this in your implementation contracts
abstract contract PreGuardUUPSUpgradeable {
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
    error UpgradeCallFailed();
    error InvalidAddress();

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
                    revert UpgradeCallFailed();
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
        if (newAdmin == address(0)) revert InvalidAddress();
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
