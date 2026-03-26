// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import {UUPSProxy, UUPSUpgradeable, StorageSlot} from "../src/Upgradeable.sol";
import {DeployTEEMLVerifierHelper} from "./helpers/DeployTEEMLVerifier.sol";

/// @title TEEMLVerifierProxyTest
/// @notice Tests for UUPS proxy deployment, initialization, upgrade, and access control
contract TEEMLVerifierProxyTest is Test, DeployTEEMLVerifierHelper {
    event Upgraded(address indexed implementation);

    TEEMLVerifier implementation;
    TEEMLVerifier verifier; // proxy cast as verifier
    address admin = address(0xAD);
    address user = address(0xBEEF);
    address remainderAddr = address(0x1234);

    function setUp() public {
        implementation = new TEEMLVerifier();
        UUPSProxy proxy = new UUPSProxy(
            address(implementation), abi.encodeCall(TEEMLVerifier.initialize, (admin, remainderAddr))
        );
        verifier = TEEMLVerifier(payable(address(proxy)));
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
        verifier.initialize(user, remainderAddr);
    }

    function test_initialize_sets_remainder_verifier() public view {
        assertEq(verifier.remainderVerifier(), remainderAddr);
    }

    function test_initialize_sets_default_config() public view {
        assertEq(verifier.challengeWindow(), 1 hours);
        assertEq(verifier.disputeWindow(), 24 hours);
        assertEq(verifier.challengeBondAmount(), 0.1 ether);
        assertEq(verifier.proverStake(), 0.1 ether);
    }

    // ========================================================================
    // UPGRADE
    // ========================================================================

    function test_upgrade_by_admin() public {
        TEEMLVerifier newImpl = new TEEMLVerifier();

        vm.prank(admin);
        verifier.upgradeTo(address(newImpl));

        assertEq(verifier.implementation(), address(newImpl));
    }

    function test_upgrade_reverts_for_non_admin() public {
        TEEMLVerifier newImpl = new TEEMLVerifier();

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
        TEEMLVerifier newImpl = new TEEMLVerifier();

        vm.prank(admin);
        verifier.upgradeToAndCall(address(newImpl), "");

        assertEq(verifier.implementation(), address(newImpl));
    }

    function test_upgrade_preserves_state() public {
        // Register an enclave, then upgrade, verify state persists
        vm.prank(admin);
        address enclaveKey = address(0xE1);
        bytes32 imageHash = bytes32(uint256(0xABCD));
        verifier.registerEnclave(enclaveKey, imageHash);

        // Verify enclave is registered
        ITEEMLVerifier.EnclaveInfo memory info = verifier.enclaves(enclaveKey);
        assertTrue(info.registered);
        assertTrue(info.active);
        assertEq(info.enclaveImageHash, imageHash);

        // Update a config value to verify it persists too
        vm.prank(admin);
        verifier.setChallengeWindow(2 hours);
        assertEq(verifier.challengeWindow(), 2 hours);

        // Upgrade
        TEEMLVerifier newImpl = new TEEMLVerifier();
        vm.prank(admin);
        verifier.upgradeTo(address(newImpl));

        // State should persist across upgrade
        ITEEMLVerifier.EnclaveInfo memory infoAfter = verifier.enclaves(enclaveKey);
        assertTrue(infoAfter.registered);
        assertTrue(infoAfter.active);
        assertEq(infoAfter.enclaveImageHash, imageHash);
        assertEq(verifier.challengeWindow(), 2 hours);
        assertEq(verifier.remainderVerifier(), remainderAddr);
    }

    function test_upgrade_emits_event() public {
        TEEMLVerifier newImpl = new TEEMLVerifier();

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

    function test_registerEnclave_through_proxy() public {
        address enclaveKey = address(0xE2);
        bytes32 imageHash = bytes32(uint256(0x9999));

        vm.prank(admin);
        verifier.registerEnclave(enclaveKey, imageHash);

        ITEEMLVerifier.EnclaveInfo memory info = verifier.enclaves(enclaveKey);
        assertTrue(info.registered);
        assertTrue(info.active);
        assertEq(info.enclaveImageHash, imageHash);
    }

    function test_receive_ether() public {
        (bool ok,) = address(verifier).call{value: 1 ether}("");
        assertTrue(ok);
    }

    // ========================================================================
    // DEPLOY HELPER
    // ========================================================================

    function test_deploy_helper_works() public {
        TEEMLVerifier v = _deployTEEMLVerifier(admin, remainderAddr);
        assertEq(v.admin(), admin);
        assertEq(v.remainderVerifier(), remainderAddr);
    }
}
