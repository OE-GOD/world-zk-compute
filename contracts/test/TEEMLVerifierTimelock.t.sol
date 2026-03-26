// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/tee/TEEMLVerifier.sol";
import "../src/TimelockController.sol";
import {UUPSUpgradeable} from "../src/Upgradeable.sol";
import {DeployTEEMLVerifierHelper} from "./helpers/DeployTEEMLVerifier.sol";

/// @title TEEMLVerifierTimelockTest
/// @notice Tests for timelock integration with TEEMLVerifier admin functions
contract TEEMLVerifierTimelockTest is Test, DeployTEEMLVerifierHelper {
    TEEMLVerifier public verifier;
    TimelockController public timelockController;

    address public admin = address(0xAD);
    address public nonOwner = address(0xDEAD);
    address public remainderAddr = address(0x1234);
    uint256 public constant TIMELOCK_DELAY = 2 days;

    event TimelockChanged(address previousTimelock, address newTimelock);

    function setUp() public {
        verifier = _deployTEEMLVerifier(admin, remainderAddr);
        vm.prank(admin);
        timelockController = new TimelockController(TIMELOCK_DELAY, admin);
    }

    // ========================================================================
    // TIMELOCK SETUP
    // ========================================================================

    function test_setTimelock_success() public {
        vm.prank(admin);
        vm.expectEmit(true, true, true, true);
        emit TimelockChanged(address(0), address(timelockController));
        verifier.setTimelock(address(timelockController));
        assertEq(verifier.timelock(), address(timelockController));
    }

    function test_setTimelock_clearTimelock() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        vm.prank(admin);
        verifier.setTimelock(address(0));
        assertEq(verifier.timelock(), address(0));
    }

    function test_setTimelock_nonAdminReverts() public {
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(UUPSUpgradeable.NotAdmin.selector));
        verifier.setTimelock(address(timelockController));
    }

    function test_timelockInitiallyZero() public view {
        assertEq(verifier.timelock(), address(0));
    }

    // ========================================================================
    // WITHOUT TIMELOCK: Admin can call timelocked functions directly
    // ========================================================================

    function test_withoutTimelock_adminCanSetRemainderVerifier() public {
        address newVerifier = address(0x5678);
        vm.prank(admin);
        verifier.setRemainderVerifier(newVerifier);
        assertEq(verifier.remainderVerifier(), newVerifier);
    }

    function test_withoutTimelock_adminCanSetChallengeBond() public {
        vm.prank(admin);
        verifier.setChallengeBondAmount(0.2 ether);
        assertEq(verifier.challengeBondAmount(), 0.2 ether);
    }

    function test_withoutTimelock_adminCanSetProverStake() public {
        vm.prank(admin);
        verifier.setProverStake(0.5 ether);
        assertEq(verifier.proverStake(), 0.5 ether);
    }

    function test_withoutTimelock_nonAdminCannotSetRemainderVerifier() public {
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(UUPSUpgradeable.NotAdmin.selector));
        verifier.setRemainderVerifier(address(0x5678));
    }

    // ========================================================================
    // WITH TIMELOCK: Admin blocked from timelocked functions
    // ========================================================================

    function test_withTimelock_adminCannotSetRemainderVerifierDirectly() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        verifier.setRemainderVerifier(address(0x5678));
    }

    function test_withTimelock_adminCannotSetChallengeBondDirectly() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        verifier.setChallengeBondAmount(0.2 ether);
    }

    function test_withTimelock_adminCannotSetProverStakeDirectly() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        verifier.setProverStake(0.5 ether);
    }

    // ========================================================================
    // WITH TIMELOCK: Timelock can execute after delay
    // ========================================================================

    function test_withTimelock_timelockCanSetRemainderVerifierAfterDelay() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));

        address newVerifier = address(0x5678);
        bytes memory callData = abi.encodeCall(TEEMLVerifier.setRemainderVerifier, (newVerifier));
        bytes32 opId = keccak256("set-remainder-verifier-op");

        vm.prank(admin);
        timelockController.schedule(opId, address(verifier), callData, TIMELOCK_DELAY);

        // Should not be executable yet
        vm.expectRevert(TimelockController.OperationNotReady.selector);
        timelockController.execute(opId);

        // Warp past the delay
        vm.warp(block.timestamp + TIMELOCK_DELAY);
        timelockController.execute(opId);

        assertEq(verifier.remainderVerifier(), newVerifier);
    }

    function test_withTimelock_timelockCanSetChallengeBondAfterDelay() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));

        bytes memory callData = abi.encodeCall(TEEMLVerifier.setChallengeBondAmount, (0.5 ether));
        bytes32 opId = keccak256("set-challenge-bond-op");

        vm.prank(admin);
        timelockController.schedule(opId, address(verifier), callData, TIMELOCK_DELAY);

        vm.warp(block.timestamp + TIMELOCK_DELAY);
        timelockController.execute(opId);

        assertEq(verifier.challengeBondAmount(), 0.5 ether);
    }

    function test_withTimelock_timelockCanSetProverStakeAfterDelay() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));

        bytes memory callData = abi.encodeCall(TEEMLVerifier.setProverStake, (0.3 ether));
        bytes32 opId = keccak256("set-prover-stake-op");

        vm.prank(admin);
        timelockController.schedule(opId, address(verifier), callData, TIMELOCK_DELAY);

        vm.warp(block.timestamp + TIMELOCK_DELAY);
        timelockController.execute(opId);

        assertEq(verifier.proverStake(), 0.3 ether);
    }

    // ========================================================================
    // WITH TIMELOCK: Admin-only (non-timelocked) functions still work
    // ========================================================================

    function test_withTimelock_adminCanStillPause() public {
        vm.startPrank(admin);
        verifier.setTimelock(address(timelockController));
        verifier.pause();
        assertTrue(verifier.paused());
        verifier.unpause();
        assertFalse(verifier.paused());
        vm.stopPrank();
    }

    function test_withTimelock_adminCanStillRegisterEnclave() public {
        vm.startPrank(admin);
        verifier.setTimelock(address(timelockController));

        address enclaveKey = address(0xE1);
        bytes32 imageHash = bytes32(uint256(0xABCD));
        verifier.registerEnclave(enclaveKey, imageHash);

        ITEEMLVerifier.EnclaveInfo memory info = verifier.enclaves(enclaveKey);
        assertTrue(info.registered);
        assertTrue(info.active);
        vm.stopPrank();
    }

    function test_withTimelock_adminCanStillRevokeEnclave() public {
        vm.startPrank(admin);
        address enclaveKey = address(0xE1);
        verifier.registerEnclave(enclaveKey, bytes32(uint256(0xABCD)));
        verifier.setTimelock(address(timelockController));

        verifier.revokeEnclave(enclaveKey);
        ITEEMLVerifier.EnclaveInfo memory info = verifier.enclaves(enclaveKey);
        assertFalse(info.active);
        vm.stopPrank();
    }

    function test_withTimelock_adminCanStillSetChallengeWindow() public {
        vm.startPrank(admin);
        verifier.setTimelock(address(timelockController));
        verifier.setChallengeWindow(2 hours);
        assertEq(verifier.challengeWindow(), 2 hours);
        vm.stopPrank();
    }

    function test_withTimelock_adminCanStillSetDisputeWindow() public {
        vm.startPrank(admin);
        verifier.setTimelock(address(timelockController));
        verifier.setDisputeWindow(48 hours);
        assertEq(verifier.disputeWindow(), 48 hours);
        vm.stopPrank();
    }
}
