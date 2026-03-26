// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.20;

import "forge-std/Test.sol";
import "../src/remainder/RemainderVerifier.sol";
import "../src/TimelockController.sol";
import {UUPSUpgradeable} from "../src/Upgradeable.sol";
import {DeployRemainderVerifierHelper} from "./helpers/DeployRemainderVerifier.sol";

contract RemainderVerifierTimelockTest is Test, DeployRemainderVerifierHelper {
    RemainderVerifier public verifier;
    TimelockController public timelockController;

    address public admin = address(0xAD);
    address public nonOwner = address(0xDEAD);
    uint256 public constant TIMELOCK_DELAY = 2 days;

    bytes32 public circuitHash = keccak256("test-circuit");

    event TimelockChanged(address previousTimelock, address newTimelock);

    function setUp() public {
        verifier = _deployRemainderVerifier(admin);
        vm.prank(admin);
        timelockController = new TimelockController(TIMELOCK_DELAY, admin);
    }

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

    function test_withoutTimelock_adminCanRegisterCircuit() public {
        (uint256[] memory sizes, uint8[] memory types, bool[] memory committed) = _makeCircuitParams(2);
        types[0] = 3;
        types[1] = 0;
        vm.prank(admin);
        verifier.registerCircuit(circuitHash, 2, sizes, types, committed, "test-circuit");
        assertTrue(verifier.isCircuitActive(circuitHash));
    }

    function test_withoutTimelock_adminCanSetGroth16Verifier() public {
        vm.prank(admin);
        verifier.setGroth16Verifier(address(0x1234));
        assertEq(verifier.groth16Verifier(), address(0x1234));
    }

    function test_withoutTimelock_nonAdminCannotRegisterCircuit() public {
        (uint256[] memory sizes, uint8[] memory types, bool[] memory committed) = _makeCircuitParams(2);
        types[0] = 3;
        types[1] = 0;
        vm.prank(nonOwner);
        vm.expectRevert(abi.encodeWithSelector(UUPSUpgradeable.NotAdmin.selector));
        verifier.registerCircuit(circuitHash, 2, sizes, types, committed, "test-circuit");
    }

    function test_withTimelock_adminCannotRegisterCircuitDirectly() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        (uint256[] memory sizes, uint8[] memory types, bool[] memory committed) = _makeCircuitParams(2);
        types[0] = 3;
        types[1] = 0;
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        verifier.registerCircuit(circuitHash, 2, sizes, types, committed, "test-circuit");
    }

    function test_withTimelock_adminCannotSetGroth16VerifierDirectly() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        vm.prank(admin);
        vm.expectRevert(UUPSUpgradeable.NotTimelocked.selector);
        verifier.setGroth16Verifier(address(0x1234));
    }

    function test_withTimelock_timelockCanRegisterCircuitAfterDelay() public {
        vm.prank(admin);
        verifier.setTimelock(address(timelockController));
        (uint256[] memory sizes, uint8[] memory types, bool[] memory committed) = _makeCircuitParams(2);
        types[0] = 3;
        types[1] = 0;
        bytes memory callData = abi.encodeCall(
            RemainderVerifier.registerCircuit, (circuitHash, 2, sizes, types, committed, "timelock-circuit")
        );
        bytes32 opId = keccak256("register-circuit-op");
        vm.prank(admin);
        timelockController.schedule(opId, address(verifier), callData, TIMELOCK_DELAY);
        vm.expectRevert(TimelockController.OperationNotReady.selector);
        timelockController.execute(opId);
        vm.warp(block.timestamp + TIMELOCK_DELAY);
        timelockController.execute(opId);
        assertTrue(verifier.isCircuitActive(circuitHash));
    }

    function test_withTimelock_adminCanStillPause() public {
        vm.startPrank(admin);
        verifier.setTimelock(address(timelockController));
        verifier.pause();
        assertTrue(verifier.paused());
        verifier.unpause();
        assertFalse(verifier.paused());
        vm.stopPrank();
    }

    function test_withTimelock_adminCanStillDeactivateCircuit() public {
        (uint256[] memory sizes, uint8[] memory types, bool[] memory committed) = _makeCircuitParams(2);
        types[0] = 3;
        types[1] = 0;
        vm.startPrank(admin);
        verifier.registerCircuit(circuitHash, 2, sizes, types, committed, "test-circuit");
        verifier.setTimelock(address(timelockController));
        verifier.deactivateCircuit(circuitHash);
        assertFalse(verifier.isCircuitActive(circuitHash));
        verifier.reactivateCircuit(circuitHash);
        assertTrue(verifier.isCircuitActive(circuitHash));
        vm.stopPrank();
    }

    function test_timelockInitiallyZero() public view {
        assertEq(verifier.timelock(), address(0));
    }

    function _makeCircuitParams(uint256 numLayers)
        internal
        pure
        returns (uint256[] memory sizes, uint8[] memory types, bool[] memory committed)
    {
        sizes = new uint256[](numLayers);
        types = new uint8[](numLayers);
        committed = new bool[](numLayers);
        for (uint256 i = 0; i < numLayers; i++) {
            sizes[i] = 4;
        }
    }
}
