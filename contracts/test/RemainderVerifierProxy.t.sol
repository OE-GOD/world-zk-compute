// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import {UUPSProxy, UUPSUpgradeable, StorageSlot} from "../src/Upgradeable.sol";

/// @title RemainderVerifierProxyTest
/// @notice Tests for UUPS proxy deployment, initialization, upgrade, and access control
contract RemainderVerifierProxyTest is Test {
    event Upgraded(address indexed implementation);

    RemainderVerifier implementation;
    RemainderVerifier verifier; // proxy cast as verifier
    address admin = address(0xAD);
    address user = address(0xBEEF);

    function setUp() public {
        implementation = new RemainderVerifier();
        UUPSProxy proxy = new UUPSProxy(address(implementation), abi.encodeCall(RemainderVerifier.initialize, (admin)));
        verifier = RemainderVerifier(payable(address(proxy)));
    }

    // ========================================================================
    // INITIALIZATION
    // ========================================================================

    function test_proxy_admin_set() public view {
        assertEq(verifier.admin(), admin);
    }

    function test_proxy_implementation_set() public view {
        assertEq(verifier.implementation(), address(implementation));
    }

    function test_proxy_proxiableUUID() public view {
        assertEq(verifier.proxiableUUID(), StorageSlot.IMPLEMENTATION_SLOT);
    }

    function test_initialize_cannot_be_called_twice() public {
        vm.expectRevert();
        verifier.initialize(user);
    }

    // ========================================================================
    // UPGRADE
    // ========================================================================

    function test_upgrade_by_admin() public {
        RemainderVerifier newImpl = new RemainderVerifier();

        vm.prank(admin);
        verifier.upgradeTo(address(newImpl));

        assertEq(verifier.implementation(), address(newImpl));
    }

    function test_upgrade_reverts_for_non_admin() public {
        RemainderVerifier newImpl = new RemainderVerifier();

        vm.prank(user);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.upgradeTo(address(newImpl));
    }

    function test_upgrade_reverts_for_zero_code_address() public {
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.InvalidImplementation.selector);
        verifier.upgradeTo(address(0xDEAD));
    }

    function test_upgradeToAndCall() public {
        RemainderVerifier newImpl = new RemainderVerifier();

        vm.prank(admin);
        verifier.upgradeToAndCall(address(newImpl), "");

        assertEq(verifier.implementation(), address(newImpl));
    }

    function test_upgrade_preserves_state() public {
        // Register a circuit, then upgrade, verify state persists
        vm.prank(admin);
        uint256[] memory sizes = new uint256[](2);
        sizes[0] = 4;
        sizes[1] = 2;
        uint8[] memory types = new uint8[](2);
        types[0] = 0;
        types[1] = 1;
        bool[] memory committed = new bool[](2);
        committed[0] = false;
        committed[1] = true;
        verifier.registerCircuit(bytes32(uint256(42)), 2, sizes, types, committed, "TestCircuit");

        // Verify circuit is registered
        (bytes32 hash,,,,) = verifier.circuits(bytes32(uint256(42)));
        assertEq(hash, bytes32(uint256(42)));

        // Upgrade
        RemainderVerifier newImpl = new RemainderVerifier();
        vm.prank(admin);
        verifier.upgradeTo(address(newImpl));

        // State should persist across upgrade
        (bytes32 hashAfter,,,,) = verifier.circuits(bytes32(uint256(42)));
        assertEq(hashAfter, bytes32(uint256(42)));
    }

    function test_upgrade_emits_event() public {
        RemainderVerifier newImpl = new RemainderVerifier();

        vm.prank(admin);
        vm.expectEmit(true, false, false, false);
        emit Upgraded(address(newImpl));
        verifier.upgradeTo(address(newImpl));
    }

    // ========================================================================
    // ADMIN FUNCTIONS WORK THROUGH PROXY
    // ========================================================================

    function test_pause_through_proxy() public {
        vm.prank(admin);
        verifier.pause();
        assertTrue(verifier.paused());

        vm.prank(admin);
        verifier.unpause();
        assertFalse(verifier.paused());
    }

    function test_changeAdmin_through_proxy() public {
        vm.prank(admin);
        verifier.changeAdmin(user);

        assertEq(verifier.admin(), user);
    }

    function test_changeAdmin_reverts_for_non_admin() public {
        vm.prank(user);
        vm.expectRevert(UUPSUpgradeable.NotAdmin.selector);
        verifier.changeAdmin(user);
    }

    // ========================================================================
    // PROXY DELEGATION
    // ========================================================================

    function test_getCircuitHashes_through_proxy() public view {
        bytes32[] memory hashes = verifier.getCircuitHashes();
        assertEq(hashes.length, 0);
    }

    function test_receive_ether() public {
        (bool ok,) = address(verifier).call{value: 1 ether}("");
        assertTrue(ok);
    }
}
