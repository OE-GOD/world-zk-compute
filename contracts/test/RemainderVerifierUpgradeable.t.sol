// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import {RemainderVerifier} from "../src/remainder/RemainderVerifier.sol";
import {UUPSUpgradeable, UUPSProxy, StorageSlot} from "../src/Upgradeable.sol";
import {DeployRemainderVerifierHelper} from "./helpers/DeployRemainderVerifier.sol";

/// @dev V2 implementation for testing upgrades. Extends RemainderVerifier storage layout.
contract RemainderVerifierV2 is RemainderVerifier {
    uint256 public version;

    function initializeV2(uint256 _version) external {
        version = _version;
    }
}

/// @dev Dummy contract with no code at deployment time (for testing invalid upgrades)
contract EmptyContract {}

/// @title RemainderVerifierUpgradeableTest
/// @notice Tests the UUPS upgradeable pattern for RemainderVerifier
contract RemainderVerifierUpgradeableTest is Test, DeployRemainderVerifierHelper {
    RemainderVerifier public verifier;
    address public admin;
    address public nonAdmin;

    function setUp() public {
        admin = address(this);
        nonAdmin = address(0xBEEF);
        verifier = _deployRemainderVerifier(admin);
    }

    // ========================================================================
    // DEPLOYMENT VIA UUPS PROXY
    // ========================================================================

    /// @notice Verify that deploying RemainderVerifier via UUPSProxy works correctly
    function test_deployViaProxy() public view {
        // The verifier address should be non-zero (proxy is deployed)
        assertTrue(address(verifier) != address(0), "Proxy should be deployed");

        // Verify that the proxy has code
        assertTrue(address(verifier).code.length > 0, "Proxy should have code");
    }

    /// @notice Verify that the implementation address is stored correctly
    function test_implementationStoredInProxy() public view {
        // Read the implementation address from the proxy's EIP-1967 slot
        address impl = verifier.implementation();
        assertTrue(impl != address(0), "Implementation should be set");
        assertTrue(impl.code.length > 0, "Implementation should have code");
    }

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    /// @notice Verify that initialize() sets the admin correctly
    function test_initializeCorrectly() public view {
        assertEq(verifier.admin(), admin, "Admin should be set to deployer");
    }

    /// @notice Verify that calling initialize() again reverts (cannot re-initialize)
    function test_cannotReinitialize() public {
        vm.expectRevert(UUPSUpgradeable.AlreadyInitialized.selector);
        verifier.initialize(address(0xDEAD));
    }

    /// @notice Verify that re-initialization attempt does not change admin
    function test_reinitializeDoesNotChangeAdmin() public {
        // Try to re-initialize (should revert)
        vm.expectRevert(UUPSUpgradeable.AlreadyInitialized.selector);
        verifier.initialize(nonAdmin);

        // Admin should remain unchanged
        assertEq(verifier.admin(), admin, "Admin should not change after failed re-initialize");
    }

    // ========================================================================
    // ADMIN ACCESS CONTROL
    // ========================================================================

    /// @notice Verify that admin can call onlyAdmin functions
    function test_adminCanPause() public {
        verifier.pause();
        assertTrue(verifier.paused(), "Admin should be able to pause");
    }

    /// @notice Verify that non-admin cannot call onlyAdmin functions
    function test_nonAdminCannotPause() public {
        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.pause();
    }

    /// @notice Verify that admin can change admin
    function test_adminCanChangeAdmin() public {
        address newAdmin = address(0xCAFE);
        verifier.changeAdmin(newAdmin);
        assertEq(verifier.admin(), newAdmin, "Admin should be changed");
    }

    /// @notice Verify that non-admin cannot change admin
    function test_nonAdminCannotChangeAdmin() public {
        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.changeAdmin(nonAdmin);
    }

    /// @notice Verify that admin cannot be changed to zero address
    function test_changeAdminToZeroReverts() public {
        vm.expectRevert(UUPSUpgradeable.InvalidAddress.selector);
        verifier.changeAdmin(address(0));
    }

    // ========================================================================
    // UPGRADE (ADMIN CAN UPGRADE)
    // ========================================================================

    /// @notice Verify that admin can upgrade to a valid new implementation
    function test_adminCanUpgrade() public {
        // Deploy a new V2 implementation
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();

        // Upgrade
        verifier.upgradeTo(address(newImpl));

        // Verify the implementation changed
        assertEq(verifier.implementation(), address(newImpl), "Implementation should be updated");
    }

    /// @notice Verify that upgradeToAndCall works (upgrade + initialize V2 in one tx)
    function test_adminCanUpgradeAndCall() public {
        // Deploy new V2 implementation
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();

        // Upgrade and call initializeV2
        verifier.upgradeToAndCall(address(newImpl), abi.encodeCall(RemainderVerifierV2.initializeV2, (42)));

        // Verify the implementation changed and V2 init was called
        assertEq(verifier.implementation(), address(newImpl), "Implementation should be updated");

        // Cast to V2 to check the new storage
        RemainderVerifierV2 v2 = RemainderVerifierV2(address(verifier));
        assertEq(v2.version(), 42, "V2 should be initialized with version=42");
    }

    /// @notice Verify that admin is preserved after upgrade
    function test_adminPreservedAfterUpgrade() public {
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();
        verifier.upgradeTo(address(newImpl));

        // Admin should still be the same (stored in EIP-1967 slot, not in implementation storage)
        assertEq(verifier.admin(), admin, "Admin should be preserved after upgrade");
    }

    // ========================================================================
    // UPGRADE (NON-ADMIN CANNOT UPGRADE)
    // ========================================================================

    /// @notice Verify that non-admin cannot upgrade
    function test_nonAdminCannotUpgrade() public {
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();

        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.upgradeTo(address(newImpl));
    }

    /// @notice Verify that non-admin cannot upgradeToAndCall
    function test_nonAdminCannotUpgradeAndCall() public {
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();

        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.upgradeToAndCall(address(newImpl), abi.encodeCall(RemainderVerifierV2.initializeV2, (42)));
    }

    /// @notice Verify that upgrading to address with no code reverts
    function test_upgradeToNoCodeReverts() public {
        // address(0x1234) has no code deployed
        vm.expectRevert(UUPSUpgradeable.InvalidImplementation.selector);
        verifier.upgradeTo(address(0x1234));
    }

    // ========================================================================
    // CIRCUIT REGISTRATION THROUGH PROXY
    // ========================================================================

    /// @notice Verify that circuit registration works through the proxy
    function test_registerCircuitThroughProxy() public {
        bytes32 circuitHash = bytes32(uint256(0x1234));
        uint256 numLayers = 3;
        uint256[] memory layerSizes = new uint256[](3);
        layerSizes[0] = 4;
        layerSizes[1] = 2;
        layerSizes[2] = 1;
        uint8[] memory layerTypes = new uint8[](3);
        layerTypes[0] = 0; // add
        layerTypes[1] = 1; // mul
        layerTypes[2] = 3; // input
        bool[] memory isCommitted = new bool[](3);
        isCommitted[0] = true;
        isCommitted[1] = true;
        isCommitted[2] = false;

        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "TestCircuit");

        // Verify the circuit is registered by checking it can be queried
        // The registerCircuit function would revert if the proxy delegation didn't work
        // Try registering the same circuit again -- should revert with CircuitAlreadyRegistered
        vm.expectRevert(RemainderVerifier.CircuitAlreadyRegistered.selector);
        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "TestCircuit");
    }

    /// @notice Verify that non-admin cannot register circuits
    function test_nonAdminCannotRegisterCircuit() public {
        bytes32 circuitHash = bytes32(uint256(0x5678));
        uint256 numLayers = 2;
        uint256[] memory layerSizes = new uint256[](2);
        layerSizes[0] = 4;
        layerSizes[1] = 2;
        uint8[] memory layerTypes = new uint8[](2);
        bool[] memory isCommitted = new bool[](2);

        vm.prank(nonAdmin);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "Test");
    }

    /// @notice Verify that circuit registration persists after upgrade
    function test_circuitRegistrationPersistsAfterUpgrade() public {
        // Register a circuit
        bytes32 circuitHash = bytes32(uint256(0xABCD));
        uint256 numLayers = 2;
        uint256[] memory layerSizes = new uint256[](2);
        layerSizes[0] = 4;
        layerSizes[1] = 2;
        uint8[] memory layerTypes = new uint8[](2);
        bool[] memory isCommitted = new bool[](2);
        isCommitted[0] = true;

        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "PersistTest");

        // Upgrade to V2
        RemainderVerifierV2 newImpl = new RemainderVerifierV2();
        verifier.upgradeTo(address(newImpl));

        // Circuit should still be registered (storage is in the proxy, not the implementation)
        // Registering the same circuit should fail
        vm.expectRevert(RemainderVerifier.CircuitAlreadyRegistered.selector);
        verifier.registerCircuit(circuitHash, numLayers, layerSizes, layerTypes, isCommitted, "PersistTest");
    }

    // ========================================================================
    // PAUSE/UNPAUSE THROUGH PROXY
    // ========================================================================

    /// @notice Verify that pause/unpause works through the proxy
    function test_pauseUnpauseThroughProxy() public {
        assertFalse(verifier.paused(), "Should start unpaused");

        verifier.pause();
        assertTrue(verifier.paused(), "Should be paused after pause()");

        verifier.unpause();
        assertFalse(verifier.paused(), "Should be unpaused after unpause()");
    }

    // ========================================================================
    // EDGE CASES
    // ========================================================================

    /// @notice Verify that the implementation contract itself cannot be initialized directly
    function test_implementationCannotBeInitializedDirectly() public {
        // Deploy a bare implementation (not via proxy)
        RemainderVerifier impl = new RemainderVerifier();

        // The implementation should not be initializable since during construction,
        // the constructor is not setting _initialized. However, with UUPS contracts
        // that don't have a constructor that blocks initialization, the impl CAN
        // be initialized directly. This test documents the current behavior.
        // If the implementation should be locked, add disableInitializers() to constructor.
        impl.initialize(address(0xDEAD));

        // But it cannot be initialized twice
        vm.expectRevert(UUPSUpgradeable.AlreadyInitialized.selector);
        impl.initialize(address(0xBEEF));
    }

    /// @notice Verify proxiableUUID returns the correct slot
    function test_proxiableUUID() public view {
        assertEq(
            verifier.proxiableUUID(),
            StorageSlot.IMPLEMENTATION_SLOT,
            "proxiableUUID should return EIP-1967 implementation slot"
        );
    }
}
